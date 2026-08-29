//! `xmltree::Element` is a plain data struct, so it maps onto an interpreter struct 1 to 1. `write`
//! rebuilds a real `Element` and lets xmltree serialize it, so output bytes match the compiled crate.

use anyhow::{Result, bail};
use xmltree::{Element, Namespace, XMLNode};

use indexmap::IndexMap;

use super::bytecode::{BuiltinId, MethodName};
use super::enum_def::XML_NODE;
use super::std_bridge::as_i64;
use super::value::{StructData, Value};

/// `Element::parse(bytes_or_str)`
pub(super) fn parse(args: &[Value]) -> Value {
    let bytes = arg_bytes(args.first());
    match Element::parse(bytes.as_slice()) {
        Ok(el) => Value::ok(element_to_value(&el)),
        Err(e) => Value::err(Value::str(e.to_string())),
    }
}

/// `Element::new(name)`
pub(super) fn new_element(name: &str) -> Value {
    element_to_value(&Element::new(name))
}

pub(super) fn element_method(
    recv: &StructData,
    name: &MethodName,
    args: &[Value],
) -> Result<Value> {
    match name.id {
        // scripts hand in a shared `Vec<u8>`, so the bytes land in the caller's vec
        BuiltinId::Write => {
            let el = value_to_element(recv)?;
            let mut out: Vec<u8> = Vec::new();
            match el.write(&mut out) {
                Ok(()) => {
                    if let Some(Value::Vec(v)) = args.first() {
                        v.lock()
                            .extend(out.into_iter().map(|b| Value::Int(i64::from(b))));
                    }
                    Ok(Value::ok(Value::Unit))
                }
                Err(e) => Ok(Value::err(Value::str(e.to_string()))),
            }
        }
        // `Option<Cow<str>>` of the direct text and cdata children
        BuiltinId::GetText => {
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
    Value::enum_named(&XML_NODE, variant, data).expect("every xmltree node variant is listed")
}

fn value_to_element(s: &StructData) -> Result<Element> {
    let namespaces = match s.get("namespaces") {
        Some(v) => match option_value(&v) {
            Some(Value::Map(m, _)) => {
                let map = m
                    .lock()
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
    if let Some(Value::Map(m, _)) = s.get("attributes") {
        for (k, v) in m.lock().iter() {
            attributes.insert(k.to_value().display(), v.display());
        }
    }
    let mut children = Vec::new();
    if let Some(Value::Vec(items)) = s.get("children") {
        for node in items.lock().iter() {
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
    let Value::Enum { def, variant, data } = v else {
        bail!("an Element child must be an XMLNode");
    };
    let variant = def.variant_name(*variant);
    let data = data.lock().clone();
    let text = |i: usize| data.get(i).map(Value::display).unwrap_or_default();
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

fn field_opt_str(s: &StructData, field: &str) -> Option<String> {
    s.get(field)
        .as_ref()
        .and_then(option_value)
        .map(|v| v.display())
}

fn option_value(v: &Value) -> Option<Value> {
    v.some_payload()
}

fn map_value(pairs: impl IntoIterator<Item = (Value, Value)>) -> Value {
    let mut map = IndexMap::default();
    for (k, v) in pairs {
        if let Some(key) = k.into_key() {
            map.insert(key, v);
        }
    }
    Value::map_of(map)
}

fn arg_bytes(v: Option<&Value>) -> Vec<u8> {
    match v {
        Some(Value::Vec(items)) => items
            .lock()
            .iter()
            .filter_map(|v| as_i64(v).and_then(|n| u8::try_from(n).ok()))
            .collect(),
        Some(Value::Str(s)) => s.as_bytes().to_vec(),
        _ => Vec::new(),
    }
}
