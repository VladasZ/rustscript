//! The xmltree bridge. `xmltree::Element` is a plain data struct with public
//! fields, so it maps onto an interpreter struct one to one: a script reads
//! and edits `name`, `prefix`, `attributes`, and `children` exactly as it
//! would with the real crate, and stays valid Rust that compiles and passes
//! `rust check`. `Element::parse` builds the value tree, `write` rebuilds a
//! real `Element` and lets xmltree serialize it, so output bytes match the
//! compiled crate byte for byte. Namespace state is carried through untouched.

use anyhow::{Result, bail};
use xmltree::{Element, Namespace, XMLNode};

use super::value::Value;

/// `Element::parse(bytes_or_str)` as the real associated function.
pub(super) fn parse(args: &[Value]) -> Value {
    let bytes = arg_bytes(args.first());
    match Element::parse(bytes.as_slice()) {
        Ok(el) => Value::ok(element_to_value(&el)),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// `Element::new(name)`, an empty element exactly as the real crate builds it.
pub(super) fn new_element(name: &str) -> Value {
    element_to_value(&Element::new(name))
}

/// Methods on an `Element` struct value, mirroring the real crate.
pub(super) fn element_method(
    recv: &super::value::StructData,
    name: &str,
    args: &[Value],
) -> Result<Value> {
    match name {
        // The real write takes any writer; scripts hand in a `Vec<u8>`, which
        // is shared, so the serialized bytes land in the caller's vec.
        "write" => {
            let el = value_to_element(recv)?;
            let mut out: Vec<u8> = Vec::new();
            match el.write(&mut out) {
                Ok(()) => {
                    if let Some(Value::Vec(v)) = args.first() {
                        v.borrow_mut()
                            .extend(out.into_iter().map(|b| Value::Int(i64::from(b))));
                    }
                    Ok(Value::ok(Value::Unit))
                }
                Err(e) => Ok(Value::err(Value::str(e.to_string()))),
            }
        }
        // Option<Cow<str>> of the direct text and cdata children.
        "get_text" => {
            let el = value_to_element(recv)?;
            Ok(match el.get_text() {
                Some(text) => Value::some(Value::str(text.to_string())),
                None => Value::none(),
            })
        }
        _ => bail!("unknown method `{name}` on Element"),
    }
}

fn element_to_value(el: &Element) -> Value {
    let attributes: Vec<(Value, Value)> = el
        .attributes
        .iter()
        .map(|(k, v)| (Value::str(k.clone()), Value::str(v.clone())))
        .collect();
    let namespaces = match &el.namespaces {
        Some(ns) => Value::some(map_value(
            ns.0.iter()
                .map(|(k, v)| (Value::str(k.clone()), Value::str(v.clone()))),
        )),
        None => Value::none(),
    };
    Value::struct_of(
        "Element",
        [
            ("prefix".into(), opt_str(el.prefix.as_deref())),
            ("namespace".into(), opt_str(el.namespace.as_deref())),
            ("namespaces".into(), namespaces),
            ("name".into(), Value::str(el.name.clone())),
            ("attributes".into(), map_value(attributes)),
            (
                "children".into(),
                Value::vec(el.children.iter().map(node_to_value).collect()),
            ),
        ],
    )
}

fn node_to_value(node: &XMLNode) -> Value {
    let (variant, data) = match node {
        XMLNode::Element(el) => ("Element", vec![element_to_value(el)]),
        XMLNode::Text(t) => ("Text", vec![Value::str(t.clone())]),
        XMLNode::Comment(t) => ("Comment", vec![Value::str(t.clone())]),
        XMLNode::CData(t) => ("CData", vec![Value::str(t.clone())]),
        XMLNode::ProcessingInstruction(target, content) => (
            "ProcessingInstruction",
            vec![Value::str(target.clone()), opt_str(content.as_deref())],
        ),
    };
    Value::Enum {
        enum_name: "XMLNode".into(),
        variant: variant.into(),
        data: data.into(),
    }
}

fn value_to_element(s: &super::value::StructData) -> Result<Element> {
    let namespaces = match s.get("namespaces") {
        Some(v) => match option_value(&v) {
            Some(Value::Map(m)) => {
                let map = m
                    .borrow()
                    .iter()
                    .map(|(k, v)| (k.to_value().display(), v.display()))
                    .collect();
                Some(Namespace(map))
            }
            _ => None,
        },
        None => None,
    };
    let mut attributes = xmltree::AttributeMap::new();
    if let Some(Value::Map(m)) = s.get("attributes") {
        for (k, v) in m.borrow().iter() {
            attributes.insert(k.to_value().display(), v.display());
        }
    }
    let mut children = Vec::new();
    if let Some(Value::Vec(items)) = s.get("children") {
        for node in items.borrow().iter() {
            children.push(value_to_node(node)?);
        }
    }
    Ok(Element {
        prefix: field_opt_str(s, "prefix"),
        namespace: field_opt_str(s, "namespace"),
        namespaces,
        name: s.get("name").map(|v| v.display()).unwrap_or_default(),
        attributes,
        children,
    })
}

fn value_to_node(v: &Value) -> Result<XMLNode> {
    let Value::Enum { variant, data, .. } = v else {
        bail!("an Element child must be an XMLNode");
    };
    let text = |i: usize| data.get(i).map(|v| v.display()).unwrap_or_default();
    Ok(match &**variant {
        "Element" => match data.first() {
            Some(Value::Struct(el)) => XMLNode::Element(value_to_element(el)?),
            _ => bail!("XMLNode::Element must carry an Element"),
        },
        "Text" => XMLNode::Text(text(0)),
        "Comment" => XMLNode::Comment(text(0)),
        "CData" => XMLNode::CData(text(0)),
        "ProcessingInstruction" => {
            let content = data.get(1).and_then(option_value).map(|v| v.display());
            XMLNode::ProcessingInstruction(text(0), content)
        }
        other => bail!("unknown XMLNode variant `{other}`"),
    })
}

fn opt_str(v: Option<&str>) -> Value {
    match v {
        Some(s) => Value::some(Value::str(s.to_string())),
        None => Value::none(),
    }
}

fn field_opt_str(s: &super::value::StructData, field: &str) -> Option<String> {
    s.get(field)
        .as_ref()
        .and_then(option_value)
        .map(|v| v.display())
}

/// The payload of a `Some`, or None for `None` and anything else.
fn option_value(v: &Value) -> Option<Value> {
    match v {
        Value::Enum {
            enum_name,
            variant,
            data,
        } if &**enum_name == "Option" && &**variant == "Some" => data.first().cloned(),
        _ => None,
    }
}

fn map_value(pairs: impl IntoIterator<Item = (Value, Value)>) -> Value {
    use std::cell::RefCell;
    use std::rc::Rc;
    let mut map = super::value::Map::default();
    for (k, v) in pairs {
        if let Some(key) = k.into_key() {
            map.insert(key, v);
        }
    }
    Value::Map(Rc::new(RefCell::new(map)))
}

/// Bytes from either a `Vec<u8>` of ints or a string argument.
fn arg_bytes(v: Option<&Value>) -> Vec<u8> {
    match v {
        Some(Value::Vec(items)) => items
            .borrow()
            .iter()
            .filter_map(|v| match v {
                Value::Int(n) => Some(*n as u8),
                _ => None,
            })
            .collect(),
        Some(Value::Str(s)) => s.as_bytes().to_vec(),
        _ => Vec::new(),
    }
}
