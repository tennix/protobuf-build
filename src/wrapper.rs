// Copyright 2019 PingCAP, Inc.

use crate::GenOpt;
use quote::ToTokens;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use syn::{
    Attribute, GenericArgument, Ident, Item, ItemEnum, ItemStruct, Meta, NestedMeta, PathArguments,
    Type,
};

pub struct WrapperGen {
    input: String,
    input_file: PathBuf,
    gen_opt: GenOpt,
}

impl WrapperGen {
    pub fn new(file_name: PathBuf, gen_opt: GenOpt) -> WrapperGen {
        let input = String::from_utf8(
            fs::read(&file_name).unwrap_or_else(|_| panic!("Could not read {:?}", file_name)),
        )
        .expect("File not utf8");
        WrapperGen {
            input,
            gen_opt,
            input_file: file_name,
        }
    }

    pub fn write(&self) {
        let mut path = self.input_file.clone();
        path.set_file_name(format!(
            "wrapper_{}",
            path.file_name().unwrap().to_str().unwrap()
        ));
        let mut out = BufWriter::new(File::create(&path).expect("Could not create file"));
        self.generate(&mut out).expect("Error generating code");
    }

    fn generate<W>(&self, buf: &mut W) -> Result<(), io::Error>
    where
        W: Write,
    {
        let file = ::syn::parse_file(&self.input).expect("Could not parse file");
        writeln!(buf, "// Generated file, please don't edit manually.\n")?;
        generate_from_items(&file.items, self.gen_opt, "", buf)
    }
}

fn generate_from_items<W>(
    items: &[Item],
    gen_opt: GenOpt,
    prefix: &str,
    buf: &mut W,
) -> Result<(), io::Error>
where
    W: Write,
{
    for item in items {
        if let Item::Struct(item) = item {
            if is_message(&item.attrs) {
                generate_struct(item, gen_opt, prefix, buf)?;
            }
        } else if let Item::Enum(item) = item {
            if is_enum(&item.attrs) {
                generate_enum(item, prefix, buf)?;
            }
        } else if let Item::Mod(m) = item {
            if let Some(ref content) = m.content {
                let prefix = format!("{}{}::", prefix, m.ident);
                generate_from_items(&content.1, gen_opt, &prefix, buf)?;
            }
        }
    }
    Ok(())
}

fn generate_struct<W>(
    item: &ItemStruct,
    gen_opt: GenOpt,
    prefix: &str,
    buf: &mut W,
) -> Result<(), io::Error>
where
    W: Write,
{
    writeln!(buf, "impl {}{} {{", prefix, item.ident)?;
    if gen_opt.contains(GenOpt::NEW) {
        generate_new(&item.ident, prefix, buf)?;
    }
    generate_default_ref(&item.ident, prefix, gen_opt, buf)?;
    item.fields
        .iter()
        .filter_map(|f| {
            f.ident
                .as_ref()
                .map(|i| (i, &f.ty, FieldKind::from_attrs(&f.attrs)))
        })
        .filter_map(|(n, t, k)| k.methods(t, n))
        .map(|m| m.write_methods(buf, gen_opt))
        .collect::<Result<Vec<_>, _>>()?;
    writeln!(buf, "}}")?;
    if gen_opt.contains(GenOpt::MESSAGE) {
        generate_message_trait(&item.ident, prefix, buf)?;
    }
    Ok(())
}

fn generate_enum<W>(item: &ItemEnum, prefix: &str, buf: &mut W) -> Result<(), io::Error>
where
    W: Write,
{
    writeln!(buf, "impl {}{} {{", prefix, item.ident)?;
    writeln!(buf, "pub fn values() -> &'static [Self] {{")?;
    writeln!(
        buf,
        "static VALUES: &'static [{}{}] = &[",
        prefix, item.ident
    )?;
    for v in &item.variants {
        writeln!(buf, "{}{}::{},", prefix, item.ident, v.ident)?;
    }
    writeln!(buf, "];\nVALUES\n}}")?;
    writeln!(buf, "}}")
}

fn generate_new<W>(name: &Ident, prefix: &str, buf: &mut W) -> Result<(), io::Error>
where
    W: Write,
{
    writeln!(
        buf,
        "pub fn new_() -> {}{} {{ ::std::default::Default::default() }}",
        prefix, name,
    )
}

fn generate_default_ref<W>(
    name: &Ident,
    prefix: &str,
    gen_opt: GenOpt,
    buf: &mut W,
) -> Result<(), io::Error>
where
    W: Write,
{
    if gen_opt.contains(GenOpt::MESSAGE) {
        writeln!(
            buf,
            "#[inline] pub fn default_ref() -> &'static Self {{ ::protobuf::Message::default_instance() }}",
        )
    } else {
        writeln!(
            buf,
            "#[inline] pub fn default_ref() -> &'static Self {{
                ::lazy_static::lazy_static! {{
                    static ref INSTANCE: {0}{1} = {0}{1}::default();
                }}
                &*INSTANCE
            }}",
            prefix, name,
        )
    }
}

fn generate_message_trait<W>(name: &Ident, prefix: &str, buf: &mut W) -> Result<(), io::Error>
where
    W: Write,
{
    write!(buf, "impl ::protobuf::Clear for {}{} {{", prefix, name)?;
    writeln!(
        buf,
        "fn clear(&mut self) {{ ::prost::Message::clear(self); }}",
    )?;
    writeln!(buf, "}}")?;

    write!(buf, "impl ::protobuf::Message for {}{} {{", prefix, name)?;
    writeln!(
        buf,
        "fn compute_size(&self) -> u32 {{ ::prost::Message::encoded_len(self) as u32 }}",
    )?;
    writeln!(
        buf,
        "fn get_cached_size(&self) -> u32 {{ ::prost::Message::encoded_len(self) as u32 }}",
    )?;
    writeln!(
        buf,
        "fn as_any(&self) -> &dyn ::std::any::Any {{ self as &dyn ::std::any::Any }}",
    )?;
    writeln!(
        buf,
        "fn descriptor(&self) -> &'static ::protobuf::reflect::MessageDescriptor {{ Self::descriptor_static() }}",
    )?;
    writeln!(buf, "fn new() -> Self {{ Self::default() }}",)?;
    writeln!(
        buf,
        "fn default_instance() -> &'static {}{} {{
        ::lazy_static::lazy_static! {{
            static ref INSTANCE: {0}{1} = {0}{1}::default();
        }}
        &*INSTANCE
    }}",
        prefix, name,
    )?;
    // The only way for this to be false is if there are `required` fields, but
    // afaict, we never use that feature. In any case rust-protobuf plans to
    // always return `true` in 3.0.
    writeln!(buf, "fn is_initialized(&self) -> bool {{ true }}",)?;
    writeln!(
        buf,
        "fn write_to_with_cached_sizes(&self, _os: &mut ::protobuf::CodedOutputStream) -> ::protobuf::ProtobufResult<()> {{ unimplemented!(); }}",
    )?;
    writeln!(
        buf,
        "fn merge_from(&mut self, _is: &mut ::protobuf::CodedInputStream) -> ::protobuf::ProtobufResult<()> {{ unimplemented!(); }}",
    )?;
    writeln!(
        buf,
        "fn get_unknown_fields(&self) -> &::protobuf::UnknownFields {{ unimplemented!(); }}",
    )?;
    writeln!(
        buf,
        "fn mut_unknown_fields(&mut self) -> &mut ::protobuf::UnknownFields {{ unimplemented!(); }}",
    )?;
    writeln!(
        buf,
        "fn write_to_bytes(&self) -> ::protobuf::ProtobufResult<Vec<u8>> {{
            let mut buf = Vec::new();
            if ::prost::Message::encode(self, &mut buf).is_err() {{
                return Err(::protobuf::ProtobufError::WireError(::protobuf::error::WireError::Other));
            }}
            Ok(buf)
        }}"
    )?;
    writeln!(
        buf,
        "fn merge_from_bytes(&mut self, bytes: &[u8]) -> ::protobuf::ProtobufResult<()> {{
            if ::prost::Message::merge(self, bytes).is_err() {{
                return Err(::protobuf::ProtobufError::WireError(::protobuf::error::WireError::Other));
            }}
            Ok(())
        }}"
    )?;
    writeln!(buf, "}}")
}

const INT_TYPES: [&str; 4] = ["int32", "int64", "uint32", "uint64"];

#[derive(Clone, Eq, PartialEq, Debug, Ord, PartialOrd)]
enum FieldKind {
    Optional(Box<FieldKind>),
    Repeated,
    Message,
    Int,
    Float,
    Bool,
    Bytes,
    String,
    OneOf(String),
    Enumeration(String),
    // Fixed are not handled.
}

impl FieldKind {
    fn from_attrs(attrs: &[Attribute]) -> FieldKind {
        for a in attrs {
            if a.path.is_ident("prost") {
                if let Ok(Meta::List(list)) = a.parse_meta() {
                    let mut kinds = list
                        .nested
                        .iter()
                        .filter_map(|item| {
                            if let NestedMeta::Meta(Meta::Word(id)) = item {
                                if id == "optional" {
                                    Some(FieldKind::Optional(Box::new(FieldKind::Message)))
                                } else if id == "message" {
                                    Some(FieldKind::Message)
                                } else if id == "repeated" {
                                    Some(FieldKind::Repeated)
                                } else if id == "bytes" {
                                    Some(FieldKind::Bytes)
                                } else if id == "string" {
                                    Some(FieldKind::String)
                                } else if id == "bool" {
                                    Some(FieldKind::Bool)
                                } else if id == "float" || id == "double" {
                                    Some(FieldKind::Float)
                                } else if INT_TYPES.contains(&&*id.to_string()) {
                                    Some(FieldKind::Int)
                                } else {
                                    None
                                }
                            } else if let NestedMeta::Meta(Meta::NameValue(mnv)) = item {
                                let value = mnv.lit.clone().into_token_stream().to_string();
                                // Trim leading and trailing `"`
                                let value = value[1..value.len() - 1].to_owned();
                                if mnv.ident == "enumeration" {
                                    Some(FieldKind::Enumeration(value))
                                } else if mnv.ident == "oneof" {
                                    Some(FieldKind::OneOf(value))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();
                    kinds.sort();
                    if !kinds.is_empty() {
                        let mut iter = kinds.into_iter();
                        let mut result = iter.next().unwrap();
                        // If the type is an optional, keep looking to find the underlying type.
                        if let FieldKind::Optional(_) = result {
                            result = FieldKind::Optional(Box::new(iter.next().unwrap()));
                        }
                        return result;
                    }
                }
            }
        }
        unreachable!("Unknown field kind");
    }

    fn methods(&self, ty: &Type, ident: &Ident) -> Option<FieldMethods> {
        let mut result = FieldMethods::new(ty, ident);
        match self {
            FieldKind::Optional(fk) => {
                let unwrapped_type = unwrap_type(ty, "Option");
                let nested_methods = fk.methods(&unwrapped_type, ident).unwrap();
                let unwrapped_type = unwrapped_type.into_token_stream().to_string();

                result.override_ty = Some(match nested_methods.override_ty {
                    Some(t) => t,
                    None => unwrapped_type.clone(),
                });
                result.ref_ty = nested_methods.ref_ty;
                result.enum_set = nested_methods.enum_set;
                result.has = true;
                result.clear = Some("::std::option::Option::None".to_owned());
                result.set = Some(match &**fk {
                    FieldKind::Enumeration(_) => "::std::option::Option::Some(v as i32)".to_owned(),
                    _ => "::std::option::Option::Some(v)".to_owned(),
                });

                let as_ref = match &result.ref_ty {
                    RefType::Ref | RefType::Deref(_) => {
                        let unwrapped_type = match &**fk {
                            FieldKind::Bytes | FieldKind::Repeated => "::std::vec::Vec",
                            _ => &unwrapped_type,
                        };
                        result.mt = MethodKind::Custom(format!(
                            "if self.{}.is_none() {{
                                self.{0} = ::std::option::Option::Some({1}::default());
                            }}
                            self.{0}.as_mut().unwrap()",
                            result.name, unwrapped_type,
                        ));
                        ".as_ref()"
                    }
                    RefType::Copy => "",
                };

                let init_val = match &**fk {
                    FieldKind::Message => {
                        result.take = Some(format!(
                            "self.{}.take().unwrap_or_else({}::default)",
                            result.name, unwrapped_type,
                        ));
                        format!("{}::default_ref()", unwrapped_type,)
                    }
                    FieldKind::Bytes => {
                        result.take = Some(format!(
                            "self.{}.take().unwrap_or_else(::std::vec::Vec::new)",
                            result.name,
                        ));
                        "&[]".to_owned()
                    }
                    FieldKind::String => {
                        result.take = Some(format!(
                            "self.{}.take().unwrap_or_else(::std::string::String::new)",
                            result.name,
                        ));
                        "\"\"".to_owned()
                    }
                    FieldKind::Int | FieldKind::Enumeration(_) => "0".to_owned(),
                    FieldKind::Float => "0.".to_owned(),
                    FieldKind::Bool => "false".to_owned(),
                    _ => unimplemented!(),
                };

                result.get = Some(match &**fk {
                    FieldKind::Enumeration(t) => format!(
                        "match self.{} {{
                            Some(v) => match {}::from_i32(v) {{\
                                Some(e) => e,
                                None => panic!(\"Unknown enum variant: {{}}\", v),
                            }},
                            None => {}::default(),
                        }}",
                        result.name, t, t,
                    ),
                    _ => format!(
                        "match self.{}{} {{
                            Some(v) => v,
                            None => {},
                        }}",
                        result.name, as_ref, init_val,
                    ),
                });
            }
            FieldKind::Message => {}
            FieldKind::Int => {
                result.ref_ty = RefType::Copy;
                result.clear = Some("0".to_owned());
            }
            FieldKind::Float => {
                result.ref_ty = RefType::Copy;
                result.clear = Some("0.".to_owned());
            }
            FieldKind::Bool => {
                result.ref_ty = RefType::Copy;
                result.clear = Some("false".to_owned());
            }
            FieldKind::Repeated => {
                result.mt = MethodKind::Standard;
                result.take = Some(format!(
                    "::std::mem::replace(&mut self.{}, ::std::vec::Vec::new())",
                    result.name
                ));
                let unwrapped_type = unwrap_type(ty, "Vec").into_token_stream().to_string();
                result.ref_ty = RefType::Deref(format!("[{}]", unwrapped_type));
            }
            FieldKind::Bytes => {
                result.ref_ty = RefType::Deref("[u8]".to_owned());
                result.mt = MethodKind::Standard;
                result.take = Some(format!(
                    "::std::mem::replace(&mut self.{}, ::std::vec::Vec::new())",
                    result.name
                ));
            }
            FieldKind::String => {
                result.ref_ty = RefType::Deref("str".to_owned());
                result.mt = MethodKind::Standard;
                result.take = Some(format!(
                    "::std::mem::replace(&mut self.{}, ::std::string::String::new())",
                    result.name
                ));
            }
            FieldKind::Enumeration(enum_type) => {
                result.override_ty = Some(enum_type.clone());
                result.ref_ty = RefType::Copy;
                result.clear = Some("0".to_owned());
                result.set = Some("v as i32".to_owned());
                result.enum_set = true;
                result.get = Some(format!(
                    "match {}::from_i32(self.{}) {{\
                        Some(e) => e,
                        None => panic!(\"Unknown enum variant: {{}}\", self.{1}),
                    }}",
                    enum_type, result.name,
                ));
            }
            // There's only a few `oneof`s and they are a bit complex, so easier to
            // handle manually.
            FieldKind::OneOf(_) => return None,
        }

        Some(result)
    }
}

fn unwrap_type(ty: &Type, type_ctor: &str) -> Type {
    match ty {
        Type::Path(p) => {
            let seg = p.path.segments.iter().last().unwrap();
            assert_eq!(seg.ident, type_ctor);
            match &seg.arguments {
                PathArguments::AngleBracketed(args) => match &args.args[0] {
                    GenericArgument::Type(ty) => ty.clone(),
                    _ => unreachable!(),
                },
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
}

struct FieldMethods {
    ty: String,
    ref_ty: RefType,
    override_ty: Option<String>,
    name: Ident,
    unesc_base: String,
    has: bool,
    // None = delegate to field's `clear`
    // Some = default value
    clear: Option<String>,
    // None = set to `v`
    // Some = expression to set.
    set: Option<String>,
    enum_set: bool,
    // Some = custom getter expression.
    get: Option<String>,
    mt: MethodKind,
    take: Option<String>,
}

impl FieldMethods {
    fn new(ty: &Type, ident: &Ident) -> FieldMethods {
        let mut unesc_base = ident.to_string();
        if unesc_base.starts_with("r#") {
            unesc_base = unesc_base[2..].to_owned();
        }
        FieldMethods {
            ty: ty.clone().into_token_stream().to_string(),
            ref_ty: RefType::Ref,
            override_ty: None,
            name: ident.clone(),
            unesc_base,
            has: false,
            clear: None,
            set: None,
            enum_set: false,
            get: None,
            mt: MethodKind::None,
            take: None,
        }
    }

    fn write_methods<W>(&self, buf: &mut W, gen_opt: GenOpt) -> Result<(), io::Error>
    where
        W: Write,
    {
        // has_*
        if self.has && gen_opt.contains(GenOpt::HAS) {
            writeln!(
                buf,
                "#[inline] pub fn has_{}(&self) -> bool {{ self.{}.is_some() }}",
                self.unesc_base, self.name
            )?;
        }
        let ty = match &self.override_ty {
            Some(s) => s.clone(),
            None => self.ty.clone(),
        };
        let ref_ty = match &self.ref_ty {
            RefType::Copy => ty.clone(),
            RefType::Ref => format!("&{}", ty),
            RefType::Deref(s) => format!("&{}", s),
        };
        // clear_*
        if gen_opt.contains(GenOpt::CLEAR) {
            match &self.clear {
                Some(s) => writeln!(
                    buf,
                    "#[inline] pub fn clear_{}(&mut self) {{ self.{} = {} }}",
                    self.unesc_base, self.name, s
                )?,
                None => writeln!(
                    buf,
                    "#[inline] pub fn clear_{}(&mut self) {{ self.{}.clear(); }}",
                    self.unesc_base, self.name
                )?,
            }
        }
        // set_*
        match &self.set {
            Some(s) if !self.enum_set => writeln!(
                buf,
                "#[inline] pub fn set_{}(&mut self, v: {}) {{ self.{} = {}; }}",
                self.unesc_base, ty, self.name, s
            )?,
            None if gen_opt.contains(GenOpt::TRIVIAL_SET) => writeln!(
                buf,
                "#[inline] pub fn set_{}(&mut self, v: {}) {{ self.{} = v; }}",
                self.unesc_base, ty, self.name
            )?,
            _ => {}
        }
        // get_*
        match &self.get {
            Some(s) => writeln!(
                buf,
                "#[inline] pub fn get_{}(&self) -> {} {{ {} }}",
                self.unesc_base, ref_ty, s
            )?,
            None => {
                if gen_opt.contains(GenOpt::TRIVIAL_GET) {
                    let rf = match &self.ref_ty {
                        RefType::Copy => "",
                        _ => "&",
                    };
                    writeln!(
                        buf,
                        "#[inline] pub fn get_{}(&self) -> {} {{ {}self.{} }}",
                        self.unesc_base, ref_ty, rf, self.name
                    )?
                }
            }
        }
        // mut_*
        if gen_opt.contains(GenOpt::MUT) {
            match &self.mt {
                MethodKind::Standard => {
                    writeln!(
                        buf,
                        "#[inline] pub fn mut_{}(&mut self) -> &mut {} {{ &mut self.{} }}",
                        self.unesc_base, ty, self.name
                    )?;
                }
                MethodKind::Custom(s) => {
                    writeln!(
                        buf,
                        "#[inline] pub fn mut_{}(&mut self) -> &mut {} {{ {} }} ",
                        self.unesc_base, ty, s
                    )?;
                }
                MethodKind::None => {}
            }
        }

        // take_*
        if gen_opt.contains(GenOpt::TAKE) {
            if let Some(s) = &self.take {
                writeln!(
                    buf,
                    "#[inline] pub fn take_{}(&mut self) -> {} {{ {} }}",
                    self.unesc_base, ty, s
                )?;
            }
        }

        Ok(())
    }
}

enum RefType {
    Copy,
    Ref,
    Deref(String),
}

enum MethodKind {
    None,
    Standard,
    Custom(String),
}

fn is_message(attrs: &[Attribute]) -> bool {
    for a in attrs {
        if a.path.is_ident("derive") {
            let tts = a.tts.to_string();
            if tts.contains(":: Message") {
                return true;
            }
        }
    }
    false
}

fn is_enum(attrs: &[Attribute]) -> bool {
    for a in attrs {
        if a.path.is_ident("derive") {
            let tts = a.tts.to_string();
            if tts.contains("Enumeration") {
                return true;
            }
        }
    }
    false
}
