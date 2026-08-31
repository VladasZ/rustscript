//! Macro lowering and format specs.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use syn::punctuated::Punctuated;
use syn::{Expr, Lit};

use crate::interpreter::bytecode::{BinKind, Const, FmtSpec, MacroKind, Op, PathRef, Reg};

use std::rc::Rc;

use super::infer::MacroBody;
use super::{Compiler, inline_holes, parse_exprs, parse_matches, parse_vec_repeat};

impl Compiler<'_> {
    pub(super) fn compile_macro(&mut self, mac: &syn::Macro, dst: Reg) -> Result<()> {
        let name = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        match name.as_str() {
            "println" | "print" | "eprintln" | "eprint" | "panic" | "anyhow" | "bail"
            | "unreachable" | "todo" | "unimplemented" => {
                // a default message with no arguments, like real Rust
                let spec = match name.as_str() {
                    "unreachable" | "todo" | "unimplemented" if mac.tokens.is_empty() => {
                        let msg = match name.as_str() {
                            "todo" => "not yet implemented",
                            "unimplemented" => "not implemented",
                            _ => "internal error: entered unreachable code",
                        };
                        self.literal_fmt_spec(msg)?
                    }
                    _ => self.build_fmt_spec(mac)?,
                };
                let kind = match name.as_str() {
                    "println" => MacroKind::Println,
                    "print" => MacroKind::Print,
                    "eprintln" => MacroKind::Eprintln,
                    "eprint" => MacroKind::Eprint,
                    "anyhow" => MacroKind::Anyhow,
                    "bail" => MacroKind::Bail,
                    _ => MacroKind::Panic,
                };
                self.emit(Op::MacroCall { kind, dst, spec });
            }
            "format" => {
                let spec = self.build_fmt_spec(mac)?;
                self.emit(Op::Fmt { dst, spec });
            }
            // `write!` lowers to build the string then `write_all`, so every writer the bridge
            // supports works and the `io::Result` is real
            "write" | "writeln" => {
                let args =
                    mac.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)?;
                let mut iter = args.iter();
                let Some(mut target) = iter.next() else {
                    bail!("{name}! needs a destination as its first argument");
                };
                // `write!(&mut s, ..)` writes to `s` itself, a copy of the handle would lose it
                while let Expr::Reference(r) = target {
                    target = &r.expr;
                }
                let recv = self.compile_expr(target)?;
                let spec = self.build_fmt_spec_from(iter, name == "writeln")?;
                let text = self.alloc();
                self.emit(Op::Fmt { dst: text, spec });
                let write_all = self.add_name("write_all".to_string());
                self.emit(Op::Method {
                    dst,
                    recv,
                    name: write_all,
                    base: text,
                    argc: 1,
                });
            }
            "vec" => self.compile_vec_macro(dst, mac)?,
            "assert" => self.compile_assert_macro(dst, mac)?,
            "assert_eq" | "assert_ne" => self.compile_assert_cmp_macro(&name, dst, mac)?,
            "matches" => self.compile_matches_macro(dst, mac)?,
            "ensure" => self.compile_ensure_macro(dst, mac)?,
            "cfg" => {
                // folds to a constant for the host, like real Rust
                let meta = mac.parse_body::<syn::Meta>()?;
                self.emit(Op::LoadBool {
                    dst,
                    v: eval_cfg(&meta)?,
                });
            }
            "dbg" => {
                let args = parse_exprs(mac)?;
                let base = self.compile_args(args.iter())?;
                self.emit(Op::Dbg {
                    dst,
                    base,
                    argc: u16::try_from(args.len())?,
                });
            }
            // the path this module was loaded from, which is what the compiler stamps too
            "file" => {
                if !mac.tokens.is_empty() {
                    bail!("file! takes no arguments");
                }
                let path = self.ctx.file.clone();
                let k = self.add_const(Const::Str(path));
                self.emit(Op::LoadConst { dst, k });
            }
            "join" => self.compile_join_macro(dst, mac)?,
            other => bail!("unsupported macro: {other}!"),
        }
        Ok(())
    }

    fn compile_assert_macro(&mut self, dst: Reg, mac: &syn::Macro) -> Result<()> {
        let args = parse_exprs(mac)?;
        let cond = args
            .first()
            .ok_or_else(|| anyhow!("assert! needs a condition"))?;
        let c = self.compile_expr(cond)?;
        let ok = self.here();
        self.emit(Op::JumpIfTrue { cond: c, to: 0 });
        let p = self.add_path(PathRef::new(vec!["::assert_failed".to_string()], None));
        self.emit(Op::CallPath {
            dst,
            path: p,
            base: dst,
            argc: 0,
        });
        let end = self.mark()?;
        self.patch_jump(ok, end);
        self.emit(Op::LoadUnit { dst });
        Ok(())
    }

    fn compile_assert_cmp_macro(&mut self, name: &str, dst: Reg, mac: &syn::Macro) -> Result<()> {
        let args = parse_exprs(mac)?;
        let a = self.compile_expr(
            args.first()
                .ok_or_else(|| anyhow!("assert needs two args"))?,
        )?;
        let b = self.compile_expr(
            args.get(1)
                .ok_or_else(|| anyhow!("assert needs two args"))?,
        )?;
        let eqr = self.alloc();
        self.emit(Op::Bin {
            dst: eqr,
            a,
            b,
            op: BinKind::Eq,
        });
        let ok = self.here();
        if name == "assert_eq" {
            self.emit(Op::JumpIfTrue { cond: eqr, to: 0 });
        } else {
            self.emit(Op::JumpIfFalse { cond: eqr, to: 0 });
        }
        let p = self.add_path(PathRef::new(vec!["::assert_failed".to_string()], None));
        self.emit(Op::CallPath {
            dst,
            path: p,
            base: dst,
            argc: 0,
        });
        let end = self.mark()?;
        self.patch_jump(ok, end);
        self.emit(Op::LoadUnit { dst });
        Ok(())
    }

    fn compile_matches_macro(&mut self, dst: Reg, mac: &syn::Macro) -> Result<()> {
        let body = if let Some(body) = self.types.macro_body(mac) {
            body
        } else {
            Rc::new(MacroBody::Matches(Box::new(parse_matches(mac)?)))
        };
        let MacroBody::Matches(parts) = &*body else {
            bail!("matches! body is not a match");
        };
        let (expr, pat, guard) = &**parts;
        let scrut = self.compile_expr(expr)?;
        self.push_scope();
        let pidx = self.pattern_info(pat)?;
        self.emit(Op::TestBind {
            val: scrut,
            pat: pidx,
            dst,
        });
        if let Some(g) = guard {
            let skip = self.here();
            self.emit(Op::JumpIfFalse { cond: dst, to: 0 });
            self.compile_into(dst, g)?;
            let end = self.mark()?;
            self.patch_jump(skip, end);
        }
        self.pop_scope();
        Ok(())
    }

    fn compile_ensure_macro(&mut self, dst: Reg, mac: &syn::Macro) -> Result<()> {
        let args = parse_exprs(mac)?;
        let cond = args
            .first()
            .ok_or_else(|| anyhow!("ensure! needs a condition"))?;
        let c = self.compile_expr(cond)?;
        let ok = self.here();
        self.emit(Op::JumpIfTrue { cond: c, to: 0 });
        let msg = self.alloc();
        if let Some(m) = args.get(1) {
            self.compile_into(msg, m)?;
        } else {
            let k = self.add_const(Const::Str(Arc::from("condition failed")));
            self.emit(Op::LoadConst { dst: msg, k });
        }
        let p = self.add_path(PathRef::new(vec!["::ensure_fail".to_string()], None));
        self.emit(Op::CallPath {
            dst,
            path: p,
            base: msg,
            argc: 1,
        });
        self.emit(Op::Ret { src: dst });
        let end = self.mark()?;
        self.patch_jump(ok, end);
        self.emit(Op::LoadUnit { dst });
        Ok(())
    }

    /// Spawn everything, then await in order.
    fn compile_join_macro(&mut self, dst: Reg, mac: &syn::Macro) -> Result<()> {
        if !self.ctx.async_mode {
            bail!("`join!` is only available under #[tokio::main]");
        }
        let args = parse_exprs(mac)?;
        // all tasks must be running before any await, or nothing overlaps
        let handles: Vec<Reg> = args
            .iter()
            .map(|a| self.compile_expr(a))
            .collect::<Result<_>>()?;
        let base = self.cur().reg_top;
        for _ in &handles {
            self.alloc();
        }
        for (i, h) in handles.iter().enumerate() {
            self.emit(Op::Await {
                dst: base + Reg::try_from(i)?,
                src: *h,
            });
        }
        self.emit(Op::MakeTuple {
            dst,
            base,
            count: u16::try_from(handles.len())?,
        });
        Ok(())
    }

    /// The body the inference pass parsed, so the nodes it typed are the nodes lowered here.
    pub(super) fn macro_exprs(&self, mac: &syn::Macro) -> Result<Rc<MacroBody>> {
        if let Some(body) = self.types.macro_body(mac) {
            return Ok(body);
        }
        if let Ok(pair) = mac.parse_body_with(parse_vec_repeat)
            && mac.path.is_ident("vec")
        {
            return Ok(Rc::new(MacroBody::Repeat(Box::new(pair))));
        }
        Ok(Rc::new(MacroBody::Exprs(parse_exprs(mac)?)))
    }

    pub(super) fn compile_vec_macro(&mut self, dst: Reg, mac: &syn::Macro) -> Result<()> {
        let body = self.macro_exprs(mac)?;
        let exprs = match &*body {
            MacroBody::Repeat(pair) => {
                let val = self.compile_expr(&pair.0)?;
                let count = self.compile_expr(&pair.1)?;
                self.emit(Op::MakeArrayRepeat { dst, val, count });
                return Ok(());
            }
            MacroBody::Exprs(exprs) => exprs,
            MacroBody::Matches(..) => bail!("vec! body is not a list"),
        };
        let base = self.compile_args(exprs.iter())?;
        self.emit(Op::MakeVec {
            dst,
            base,
            count: u16::try_from(exprs.len())?,
        });
        Ok(())
    }

    /// For the no argument forms of `unreachable!`, `todo!` and `unimplemented!`.
    pub(super) fn literal_fmt_spec(&mut self, text: &str) -> Result<u16> {
        let f = self.cur();
        f.fmts.push(FmtSpec {
            template: text.to_string(),
            positional: Vec::new(),
            named: Vec::new(),
        });
        Ok(u16::try_from(f.fmts.len() - 1)?)
    }

    pub(super) fn build_fmt_spec(&mut self, mac: &syn::Macro) -> Result<u16> {
        let body = self.macro_exprs(mac)?;
        let MacroBody::Exprs(args) = &*body else {
            bail!("format arguments are not a list");
        };
        self.build_fmt_spec_from(args.iter(), false)
    }

    /// `newline` extends the template, not the value, so a bare `writeln!(f)` is a lone newline.
    pub(super) fn build_fmt_spec_from<'a>(
        &mut self,
        mut iter: impl Iterator<Item = &'a Expr>,
        newline: bool,
    ) -> Result<u16> {
        let mut template = match iter.next() {
            Some(Expr::Lit(l)) => match &l.lit {
                Lit::Str(s) => s.value(),
                _ => bail!("format template must be a string literal"),
            },
            Some(_) => bail!("format template must be a string literal"),
            None => String::new(),
        };
        if newline {
            template.push('\n');
        }
        let mut positional = Vec::new();
        let mut named: Vec<(String, Reg)> = Vec::new();
        for arg in iter {
            let value = match arg {
                Expr::Assign(a) if matches!(&*a.left, Expr::Path(p) if p.path.get_ident().is_some()) => {
                    &*a.right
                }
                other => other,
            };
            let r = self.compile_expr(value)?;
            if let Expr::Assign(a) = arg
                && let Expr::Path(p) = &*a.left
                && let Some(n) = p.path.get_ident()
            {
                named.push((n.to_string(), r));
                continue;
            }
            positional.push(r);
        }
        // inline identifiers not given explicitly
        for hole in inline_holes(&template) {
            if named.iter().all(|(n, _)| n != &hole) {
                let r = self.alloc();
                self.load_name(&hole, r)?;
                named.push((hole, r));
            }
        }
        let f = self.cur();
        f.fmts.push(FmtSpec {
            template,
            positional,
            named,
        });
        Ok(u16::try_from(f.fmts.len() - 1)?)
    }

    // jump patching
}

/// Anything unhandled is an error, a silent false would pick the wrong branch.
fn eval_cfg(meta: &syn::Meta) -> Result<bool> {
    match meta {
        syn::Meta::Path(path) => {
            let name = path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            match name.as_str() {
                "windows" => Ok(cfg!(windows)),
                "unix" => Ok(cfg!(unix)),
                "test" | "debug_assertions" | "doc" | "miri" => Ok(false),
                other => bail!("unsupported cfg predicate `{other}`"),
            }
        }
        syn::Meta::NameValue(nv) => {
            let key = nv
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            let Expr::Lit(lit) = &nv.value else {
                bail!("cfg value must be a string literal");
            };
            let Lit::Str(want) = &lit.lit else {
                bail!("cfg value must be a string literal");
            };
            let want = want.value();
            Ok(match key.as_str() {
                "target_os" => want == std::env::consts::OS,
                "target_arch" => want == std::env::consts::ARCH,
                "target_family" => want == std::env::consts::FAMILY,
                "target_pointer_width" => want == (usize::BITS).to_string(),
                other => bail!("unsupported cfg key `{other}`"),
            })
        }
        syn::Meta::List(list) => {
            let op = list
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            let inner: Punctuated<syn::Meta, syn::Token![,]> =
                list.parse_args_with(Punctuated::parse_terminated)?;
            let mut results = Vec::new();
            for m in &inner {
                results.push(eval_cfg(m)?);
            }
            match op.as_str() {
                "not" => match results.as_slice() {
                    [one] => Ok(!one),
                    _ => bail!("cfg not() takes exactly one predicate"),
                },
                "all" => Ok(results.iter().all(|r| *r)),
                "any" => Ok(results.iter().any(|r| *r)),
                other => bail!("unsupported cfg combinator `{other}`"),
            }
        }
    }
}
