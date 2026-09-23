//! `serde_json::json!`. The tokens split into objects, arrays, `null` and interpolated
//! expressions like the real macro does, and each part builds through a runtime helper. An
//! interpolated value goes through serde, so a struct lands as its object and `None` as null.

use anyhow::{Result, anyhow, bail};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens as _;

use crate::interpreter::bytecode::{Const, Op, PathRef, Reg};

use super::{Compiler, idx16};

enum Node {
    Null,
    Array(Vec<Node>),
    Object(Vec<(Key, Node)>),
    Expr(Box<syn::Expr>),
}

enum Key {
    Lit(String),
    Expr(Box<syn::Expr>),
}

impl Compiler<'_> {
    pub(super) fn compile_json_macro(&mut self, dst: Reg, mac: &syn::Macro) -> Result<()> {
        let tokens: Vec<TokenTree> = mac.tokens.clone().into_iter().collect();
        if tokens.is_empty() {
            bail!("json! needs a value");
        }
        let node = parse_value(tokens)?;
        self.compile_json_node(dst, &node)
    }

    fn compile_json_node(&mut self, dst: Reg, node: &Node) -> Result<()> {
        match node {
            Node::Null => self.json_call(dst, "::json_null", dst, 0),
            Node::Expr(e) => {
                let base = self.alloc();
                self.compile_into(base, e)?;
                self.json_call(dst, "::json_value", base, 1);
            }
            Node::Array(items) => {
                let base = self.window(items.len());
                for (i, item) in items.iter().enumerate() {
                    self.compile_json_node(base + idx16(i), item)?;
                }
                self.json_call(dst, "::json_array", base, items.len());
            }
            Node::Object(pairs) => {
                let base = self.window(pairs.len() * 2);
                for (i, (key, value)) in pairs.iter().enumerate() {
                    let at = base + idx16(i * 2);
                    match key {
                        Key::Lit(text) => {
                            let k = self.add_const(Const::Str(text.as_str().into()));
                            self.emit(Op::LoadConst { dst: at, k });
                        }
                        Key::Expr(e) => self.compile_into(at, e)?,
                    }
                    self.compile_json_node(at + 1, value)?;
                }
                self.json_call(dst, "::json_object", base, pairs.len() * 2);
            }
        }
        Ok(())
    }

    /// Registers reserved up front, so the parts built into them can't break the packing.
    fn window(&mut self, len: usize) -> Reg {
        let base = self.cur().reg_top;
        for _ in 0..len {
            self.alloc();
        }
        base
    }

    fn json_call(&mut self, dst: Reg, helper: &str, base: Reg, argc: usize) {
        let path = self.add_path(PathRef::new(vec![helper.to_string()], None));
        self.emit(Op::CallPath {
            dst,
            path,
            base,
            argc: idx16(argc),
        });
    }
}

fn parse_value(tokens: Vec<TokenTree>) -> Result<Node> {
    if let [only] = tokens.as_slice() {
        match only {
            TokenTree::Ident(id) if id == "null" => return Ok(Node::Null),
            TokenTree::Group(g) if g.delimiter() == Delimiter::Bracket => {
                let items = split_top(g.stream(), ',')
                    .into_iter()
                    .map(parse_value)
                    .collect::<Result<_>>()?;
                return Ok(Node::Array(items));
            }
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => {
                let mut pairs = Vec::new();
                for entry in split_top(g.stream(), ',') {
                    let (key, value) = split_key(entry)?;
                    pairs.push((parse_key(key)?, parse_value(value)?));
                }
                return Ok(Node::Object(pairs));
            }
            _ => {}
        }
    }
    let stream: TokenStream = tokens.into_iter().collect();
    let expr: syn::Expr =
        syn::parse2(stream).map_err(|e| anyhow!("json! value is not an expression: {e}"))?;
    Ok(Node::Expr(Box::new(expr)))
}

fn parse_key(tokens: Vec<TokenTree>) -> Result<Key> {
    if let [TokenTree::Literal(lit)] = tokens.as_slice()
        && let Ok(syn::Lit::Str(s)) = syn::parse2::<syn::Lit>(lit.clone().into_token_stream())
    {
        return Ok(Key::Lit(s.value()));
    }
    let stream: TokenStream = tokens.into_iter().collect();
    let expr: syn::Expr =
        syn::parse2(stream).map_err(|e| anyhow!("json! key is not an expression: {e}"))?;
    Ok(Key::Expr(Box::new(expr)))
}

/// The parts between top level separators, a trailing separator adds nothing. Brackets and
/// braces are single tokens, so a separator inside them is not top level.
fn split_top(stream: TokenStream, sep: char) -> Vec<Vec<TokenTree>> {
    let mut parts = vec![Vec::new()];
    for tt in stream {
        match &tt {
            TokenTree::Punct(p) if p.as_char() == sep => parts.push(Vec::new()),
            _ => {
                if let Some(last) = parts.last_mut() {
                    last.push(tt);
                }
            }
        }
    }
    parts.retain(|p| !p.is_empty());
    parts
}

/// `key: value` at the first lone `:`, a `::` belongs to a path in the key.
fn split_key(entry: Vec<TokenTree>) -> Result<(Vec<TokenTree>, Vec<TokenTree>)> {
    let is_colon =
        |t: Option<&TokenTree>| matches!(t, Some(TokenTree::Punct(p)) if p.as_char() == ':');
    for i in 0..entry.len() {
        if is_colon(entry.get(i))
            && !is_colon(entry.get(i + 1))
            && !(i > 0 && is_colon(entry.get(i - 1)))
        {
            let mut key = entry;
            let value = key.split_off(i + 1);
            key.pop();
            return Ok((key, value));
        }
    }
    bail!("json! object entry needs `key: value`")
}
