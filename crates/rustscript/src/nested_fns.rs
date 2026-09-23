//! A `fn` declared inside a block moves to the module under a hidden name, and
//! every use of its name inside that block is renamed to match. The rest of the interpreter then
//! sees a plain module function. A nested fn can't capture locals, so moving it keeps its meaning.
//!
//! Items of a block see each other, so the body of a nested fn is renamed with its siblings in
//! scope. An inner block with an item of the same name shadows the outer one.

use std::collections::HashMap;

use proc_macro2::{Group, TokenStream, TokenTree};
use syn::visit_mut::{self, VisitMut};
use syn::{Item, ItemFn, Stmt};

pub fn hoist(items: &mut Vec<Item>) {
    let mut hoister = Hoister {
        renames: HashMap::new(),
        hoisted: Vec::new(),
        counter: 0,
    };
    for item in items.iter_mut() {
        match item {
            Item::Fn(f) => hoister.visit_block_mut(&mut f.block),
            Item::Impl(imp) => {
                for member in &mut imp.items {
                    if let syn::ImplItem::Fn(m) = member {
                        hoister.visit_block_mut(&mut m.block);
                    }
                }
            }
            _ => {}
        }
    }
    items.extend(hoister.hoisted.into_iter().map(Item::Fn));
}

struct Hoister {
    /// written name to module name, for the blocks being walked
    renames: HashMap<String, String>,
    hoisted: Vec<ItemFn>,
    counter: usize,
}

impl VisitMut for Hoister {
    fn visit_block_mut(&mut self, block: &mut syn::Block) {
        let mut nested = Vec::new();
        block.stmts.retain(|stmt| match stmt {
            Stmt::Item(Item::Fn(f)) => {
                nested.push(f.clone());
                false
            }
            _ => true,
        });
        if nested.is_empty() {
            visit_mut::visit_block_mut(self, block);
            return;
        }
        let saved = self.renames.clone();
        for f in &nested {
            let written = f.sig.ident.to_string();
            self.counter += 1;
            // the written name stays in front, so a trace still shows it
            let hidden = format!("{written}__nested{}", self.counter);
            self.renames.insert(written, hidden);
        }
        for mut f in nested {
            let written = f.sig.ident.to_string();
            let hidden = self.renames[&written].clone();
            f.sig.ident = syn::Ident::new(&hidden, f.sig.ident.span());
            self.visit_block_mut(&mut f.block);
            self.hoisted.push(f);
        }
        visit_mut::visit_block_mut(self, block);
        self.renames = saved;
    }

    fn visit_expr_path_mut(&mut self, p: &mut syn::ExprPath) {
        if p.qself.is_none()
            && p.path.segments.len() == 1
            && let Some(hidden) = self.renames.get(&p.path.segments[0].ident.to_string())
        {
            let seg = &mut p.path.segments[0];
            seg.ident = syn::Ident::new(hidden, seg.ident.span());
        }
        visit_mut::visit_expr_path_mut(self, p);
    }

    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        if !self.renames.is_empty() {
            mac.tokens = rename_tokens(mac.tokens.clone(), &self.renames);
        }
        visit_mut::visit_macro_mut(self, mac);
    }
}

/// Macro arguments stay tokens until the compiler parses them, so the names are renamed there.
/// A name after `.` is a method and one after `::` belongs to a path, both keep theirs.
fn rename_tokens(tokens: TokenStream, renames: &HashMap<String, String>) -> TokenStream {
    let mut out = Vec::new();
    let mut previous: Option<TokenTree> = None;
    for tt in tokens {
        let next = match &tt {
            TokenTree::Group(g) => {
                let mut group = Group::new(g.delimiter(), rename_tokens(g.stream(), renames));
                group.set_span(g.span());
                TokenTree::Group(group)
            }
            TokenTree::Ident(id) => {
                let after_dot_or_path = matches!(
                    &previous,
                    Some(TokenTree::Punct(p)) if p.as_char() == '.' || p.as_char() == ':'
                );
                match renames.get(&id.to_string()) {
                    Some(hidden) if !after_dot_or_path => {
                        TokenTree::Ident(proc_macro2::Ident::new(hidden, id.span()))
                    }
                    _ => tt.clone(),
                }
            }
            _ => tt.clone(),
        };
        previous = Some(next.clone());
        out.push(next);
    }
    out.into_iter().collect()
}
