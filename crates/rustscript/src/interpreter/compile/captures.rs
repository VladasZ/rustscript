//! The locals the closures of a body write. A closure compiles after the statements before it,
//! so the compiler must know up front which bindings live in a capture cell. Otherwise a store
//! compiled before the closure writes the register while the closure reads the cell.

use std::collections::HashSet;

use syn::visit::{self, Visit};
use syn::{Block, Expr, Pat};

use super::is_assign_op;

/// The names the closures and `async` blocks inside `body` write, through an assignment, a
/// `&mut` borrow, a mutating method or a `write!`. A name a closure declares itself is left
/// out, that binding is the closure's own local.
pub(super) fn closure_written_names(
    body: &Block,
    mutates: &dyn Fn(&str) -> bool,
) -> HashSet<String> {
    let mut scan = Scan::new(mutates);
    scan.visit_block(body);
    scan.names
}

/// `closure_written_names` for a closure body, which is an expression.
pub(super) fn closure_written_names_expr(
    body: &Expr,
    mutates: &dyn Fn(&str) -> bool,
) -> HashSet<String> {
    let mut scan = Scan::new(mutates);
    scan.visit_expr(body);
    scan.names
}

struct Scan<'m> {
    /// closure nesting, 0 is the body itself
    depth: usize,
    names: HashSet<String>,
    /// the names each open closure declares, its parameters and `let`s
    local: Vec<HashSet<String>>,
    mutates: &'m dyn Fn(&str) -> bool,
}

impl<'m> Scan<'m> {
    fn new(mutates: &'m dyn Fn(&str) -> bool) -> Self {
        Scan {
            depth: 0,
            names: HashSet::new(),
            local: Vec::new(),
            mutates,
        }
    }

    fn written(&mut self, target: &Expr) {
        if self.depth == 0 {
            return;
        }
        let Some(name) = root_name(target) else {
            return;
        };
        self.written_name(name);
    }

    fn written_name(&mut self, name: String) {
        if self.depth == 0 || self.local.iter().any(|set| set.contains(&name)) {
            return;
        }
        self.names.insert(name);
    }

    fn declare(&mut self, pat: &Pat) {
        if let Some(set) = self.local.last_mut() {
            let mut idents = Idents(set);
            idents.visit_pat(pat);
        }
    }

    fn enter<'p>(&mut self, params: impl Iterator<Item = &'p Pat>) {
        self.depth += 1;
        self.local.push(HashSet::new());
        for p in params {
            self.declare(p);
        }
    }

    fn leave(&mut self) {
        self.local.pop();
        self.depth -= 1;
    }
}

struct Idents<'s>(&'s mut HashSet<String>);

impl Visit<'_> for Idents<'_> {
    fn visit_pat_ident(&mut self, id: &syn::PatIdent) {
        self.0.insert(id.ident.to_string());
        visit::visit_pat_ident(self, id);
    }
}

impl<'ast> Visit<'ast> for Scan<'_> {
    fn visit_expr_closure(&mut self, c: &'ast syn::ExprClosure) {
        self.enter(c.inputs.iter());
        self.visit_expr(&c.body);
        self.leave();
    }

    fn visit_expr_async(&mut self, a: &'ast syn::ExprAsync) {
        self.enter(std::iter::empty());
        self.visit_block(&a.block);
        self.leave();
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        visit::visit_local(self, local);
        if self.depth > 0 {
            self.declare(&local.pat);
        }
    }

    fn visit_expr_assign(&mut self, a: &'ast syn::ExprAssign) {
        self.written(&a.left);
        visit::visit_expr_assign(self, a);
    }

    fn visit_expr_binary(&mut self, b: &'ast syn::ExprBinary) {
        if is_assign_op(&b.op) {
            self.written(&b.left);
        }
        visit::visit_expr_binary(self, b);
    }

    fn visit_expr_reference(&mut self, r: &'ast syn::ExprReference) {
        if r.mutability.is_some() {
            self.written(&r.expr);
        }
        visit::visit_expr_reference(self, r);
    }

    fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
        if (self.mutates)(&m.method.to_string()) {
            self.written(&m.receiver);
        }
        visit::visit_expr_method_call(self, m);
    }

    /// `write!(buf, ..)` and `writeln!` mutate their first argument.
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let writes = mac
            .path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "write" || seg.ident == "writeln");
        if writes
            && let Some(proc_macro2::TokenTree::Ident(first)) =
                mac.tokens.clone().into_iter().next()
        {
            self.written_name(first.to_string());
        }
        visit::visit_macro(self, mac);
    }
}

/// Whether the closure body reads `name` only through fields that copy. A `move` closure then
/// copies those fields and leaves the value with the frame, the edition 2021 disjoint capture,
/// so the frame still drops it where it was declared.
pub(super) fn captures_only_copy_fields(
    body: &Expr,
    name: &str,
    copies: &dyn Fn(&Expr) -> bool,
) -> bool {
    let mut uses = Uses {
        name,
        copies,
        whole: false,
    };
    uses.visit_expr(body);
    !uses.whole
}

struct Uses<'a> {
    name: &'a str,
    copies: &'a dyn Fn(&Expr) -> bool,
    /// the name was read as a whole, or through a field that does not copy
    whole: bool,
}

fn is_name(expr: &Expr, name: &str) -> bool {
    matches!(expr, Expr::Path(p) if p.qself.is_none() && p.path.segments.len() == 1 && p.path.segments[0].ident == name)
}

impl<'ast> Visit<'ast> for Uses<'_> {
    fn visit_expr(&mut self, e: &'ast Expr) {
        if self.whole {
            return;
        }
        match e {
            Expr::Field(f) if is_name(&f.base, self.name) => {
                if !(self.copies)(e) {
                    self.whole = true;
                }
            }
            _ if is_name(e, self.name) => self.whole = true,
            _ => visit::visit_expr(self, e),
        }
    }

    /// The arguments of a `println!` or a `vec!` are expressions the walk can not see, so they
    /// are parsed. A mention the parse can not type counts as a read of the whole value.
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if self.whole {
            return;
        }
        let parsed = mac
            .parse_body_with(syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated);
        let Ok(args) = parsed else {
            let mentions =
                mac.tokens.clone().into_iter().any(
                    |tree| matches!(tree, proc_macro2::TokenTree::Ident(id) if id == self.name),
                );
            if mentions {
                self.whole = true;
            }
            return;
        };
        for arg in &args {
            // `{name}` and `{name:?}` inside a format string read the whole value
            if let Expr::Lit(lit) = arg
                && let syn::Lit::Str(s) = &lit.lit
                && s.value().contains(&format!("{{{}", self.name))
            {
                self.whole = true;
                return;
            }
            self.visit_expr(arg);
        }
    }
}

/// The local a place expression bottoms out in, through fields, elements, derefs and method
/// chains.
fn root_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Path(p) if p.path.segments.len() == 1 && p.qself.is_none() => {
            Some(p.path.segments[0].ident.to_string())
        }
        Expr::Paren(p) => root_name(&p.expr),
        Expr::Group(g) => root_name(&g.expr),
        Expr::Field(f) => root_name(&f.base),
        Expr::Index(i) => root_name(&i.expr),
        Expr::Reference(r) => root_name(&r.expr),
        Expr::Try(t) => root_name(&t.expr),
        Expr::MethodCall(m) => root_name(&m.receiver),
        Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => root_name(&u.expr),
        _ => None,
    }
}
