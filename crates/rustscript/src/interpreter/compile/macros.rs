//! Macro lowering and format specs. Split from the compiler.


use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use syn::punctuated::Punctuated;
use syn::{Expr, Lit};

use crate::interpreter::bytecode::{
    BinKind, Const, FmtSpec, MacroKind, Op, Reg,
};

use super::*;

impl Compiler<'_> {
    pub(super) fn compile_macro(&mut self, mac: &syn::Macro, dst: Reg) -> Result<()> {
        let name = mac.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
        match name.as_str() {
            "println" | "print" | "eprintln" | "eprint" | "panic" | "anyhow" | "bail"
            | "unreachable" | "todo" | "unimplemented" => {
                // These three abort like panic; give a default message when the
                // macro is called with no arguments, matching real Rust.
                let spec = match name.as_str() {
                    "unreachable" | "todo" | "unimplemented" if mac.tokens.is_empty() => {
                        let msg = match name.as_str() {
                            "todo" => "not yet implemented",
                            "unimplemented" => "not implemented",
                            _ => "internal error: entered unreachable code",
                        };
                        self.literal_fmt_spec(msg)
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
            "vec" => self.compile_vec_macro(dst, mac)?,
            "assert" => {
                let args = parse_exprs(mac)?;
                let cond = args.first().ok_or_else(|| anyhow!("assert! needs a condition"))?;
                let c = self.compile_expr(cond)?;
                let ok = self.here();
                self.emit(Op::JumpIfTrue { cond: c, to: 0 });
                let p = self.add_path(vec!["::assert_failed".to_string()], None);
                self.emit(Op::CallPath { dst, path: p, base: dst, argc: 0 });
                let end = self.here() as u32;
                self.patch_jump(ok, end);
                self.emit(Op::LoadUnit { dst });
            }
            "assert_eq" | "assert_ne" => {
                let args = parse_exprs(mac)?;
                let a = self.compile_expr(args.first().ok_or_else(|| anyhow!("assert needs two args"))?)?;
                let b = self.compile_expr(args.get(1).ok_or_else(|| anyhow!("assert needs two args"))?)?;
                let eqr = self.alloc();
                self.emit(Op::Bin { dst: eqr, a, b, op: BinKind::Eq });
                let ok = self.here();
                if name == "assert_eq" {
                    self.emit(Op::JumpIfTrue { cond: eqr, to: 0 });
                } else {
                    self.emit(Op::JumpIfFalse { cond: eqr, to: 0 });
                }
                let p = self.add_path(vec!["::assert_failed".to_string()], None);
                self.emit(Op::CallPath { dst, path: p, base: dst, argc: 0 });
                let end = self.here() as u32;
                self.patch_jump(ok, end);
                self.emit(Op::LoadUnit { dst });
            }
            "matches" => {
                let (expr, pat, guard) = parse_matches(mac)?;
                let scrut = self.compile_expr(&expr)?;
                self.push_scope();
                let pidx = self.pattern_info(&pat)?;
                self.emit(Op::TestBind { val: scrut, pat: pidx, dst });
                if let Some(g) = guard {
                    let skip = self.here();
                    self.emit(Op::JumpIfFalse { cond: dst, to: 0 });
                    self.compile_into(dst, &g)?;
                    let end = self.here() as u32;
                    self.patch_jump(skip, end);
                }
                self.pop_scope();
            }
            "ensure" => {
                let args = parse_exprs(mac)?;
                let cond = args.first().ok_or_else(|| anyhow!("ensure! needs a condition"))?;
                let c = self.compile_expr(cond)?;
                let ok = self.here();
                self.emit(Op::JumpIfTrue { cond: c, to: 0 });
                // Build the error message and return it.
                let msg = self.alloc();
                if let Some(m) = args.get(1) {
                    self.compile_into(msg, m)?;
                } else {
                    let k = self.add_const(Const::Str(Arc::from("condition failed")));
                    self.emit(Op::LoadConst { dst: msg, k });
                }
                let p = self.add_path(vec!["::ensure_fail".to_string()], None);
                self.emit(Op::CallPath { dst, path: p, base: msg, argc: 1 });
                self.emit(Op::Ret { src: dst });
                let end = self.here() as u32;
                self.patch_jump(ok, end);
                self.emit(Op::LoadUnit { dst });
            }
            "dbg" => {
                let args = parse_exprs(mac)?;
                let base = self.compile_args(args.iter())?;
                self.emit(Op::Dbg { dst, base, argc: args.len() as u16 });
            }
            "join" => {
                if !self.ctx.async_mode {
                    bail!("`join!` is only available under #[tokio::main]");
                }
                let args = parse_exprs(mac)?;
                // Evaluate every argument first, so all spawned tasks are running
                // before we await any of them, which is what makes join overlap.
                let handles: Vec<Reg> =
                    args.iter().map(|a| self.compile_expr(a)).collect::<Result<_>>()?;
                let base = self.cur().reg_top;
                for _ in &handles {
                    self.alloc();
                }
                for (i, h) in handles.iter().enumerate() {
                    self.emit(Op::Await { dst: base + i as Reg, src: *h });
                }
                self.emit(Op::MakeTuple { dst, base, count: handles.len() as u16 });
            }
            other => bail!("unsupported macro: {other}!"),
        }
        Ok(())
    }

    pub(super) fn compile_vec_macro(&mut self, dst: Reg, mac: &syn::Macro) -> Result<()> {
        if let Ok(rep) = mac.parse_body_with(parse_vec_repeat) {
            let val = self.compile_expr(&rep.0)?;
            let count = self.compile_expr(&rep.1)?;
            self.emit(Op::MakeArrayRepeat { dst, val, count });
            return Ok(());
        }
        let exprs = parse_exprs(mac)?;
        let base = self.compile_args(exprs.iter())?;
        self.emit(Op::MakeVec { dst, base, count: exprs.len() as u16 });
        Ok(())
    }

    /// Parse a format macro body and compile its arguments, resolving inline
    /// `{name}` holes to variables in scope.
    /// A format spec that is a fixed string with no interpolation, for the
    /// no-argument forms of `unreachable!`, `todo!`, and `unimplemented!`.
    pub(super) fn literal_fmt_spec(&mut self, text: &str) -> u16 {
        let f = self.cur();
        f.fmts.push(FmtSpec { template: text.to_string(), positional: Vec::new(), named: Vec::new() });
        (f.fmts.len() - 1) as u16
    }

    pub(super) fn build_fmt_spec(&mut self, mac: &syn::Macro) -> Result<u16> {
        let args = mac.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)?;
        let mut iter = args.iter();
        let template = match iter.next() {
            Some(Expr::Lit(l)) => match &l.lit {
                Lit::Str(s) => s.value(),
                _ => bail!("format template must be a string literal"),
            },
            Some(_) => bail!("format template must be a string literal"),
            None => String::new(),
        };
        let mut positional = Vec::new();
        let mut named: Vec<(String, Reg)> = Vec::new();
        for arg in iter {
            if let Expr::Assign(a) = arg
                && let Expr::Path(p) = &*a.left
                && let Some(n) = p.path.get_ident()
            {
                let r = self.compile_expr(&a.right)?;
                named.push((n.to_string(), r));
                continue;
            }
            let r = self.compile_expr(arg)?;
            positional.push(r);
        }
        // Inline identifiers referenced in the template but not given explicitly.
        for hole in inline_holes(&template) {
            if named.iter().all(|(n, _)| n != &hole) {
                let r = self.alloc();
                self.load_name(&hole, r)?;
                named.push((hole, r));
            }
        }
        let f = self.cur();
        f.fmts.push(FmtSpec { template, positional, named });
        Ok((f.fmts.len() - 1) as u16)
    }

    // -- jump patching -----------------------------------------------------

}
