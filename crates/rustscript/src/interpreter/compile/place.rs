//! Place aware lowering for mutable access. The chain of `Unique*` ops splits each level of the place
//! from sharing, then the mutation runs in place. See `Value::make_unique`.

use anyhow::Result;
use syn::Expr;

use super::super::bytecode::{Op, PathId, Reg};
use super::{Compiler, NameLoc};

/// Composite storage is shared with the place, so the store is a refcount move. A string splits inside
/// its methods and only the store brings the new buffer home.
pub(super) enum PlaceBack {
    /// nothing to store
    None,
    Cell(Reg),
    Upvalue(u16),
    Field {
        base: Reg,
        member: u16,
        /// so a projection whose base is a cell or another projection still lands
        parent: Box<PlaceBack>,
    },
    Index {
        base: Reg,
        key: Reg,
        /// see `Field::parent`
        parent: Box<PlaceBack>,
    },
}

/// A register sharing the now unique storage of the place, plus how to store back.
pub(super) struct Place {
    pub reg: Reg,
    pub back: PlaceBack,
}

impl Compiler<'_> {
    pub(super) fn unalias(&mut self, name: &str) -> String {
        let aliases = &self.cur().aliases;
        let mut seen = name;
        while let Some(next) = aliases.get(seen) {
            seen = next;
        }
        seen.to_string()
    }

    /// `None` for a temporary like a call result.
    pub(super) fn compile_place(&mut self, expr: &Expr) -> Result<Option<Place>> {
        match expr {
            Expr::Paren(p) => self.compile_place(&p.expr),
            // a borrow of a place is the place
            Expr::Reference(r) => self.compile_place(&r.expr),
            Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => {
                if let Some(name) = single_path_name(&u.expr) {
                    let target = self.unalias(&name);
                    if target != name {
                        return Ok(self.compile_name_place(&target));
                    }
                    // a captured alias lives in an enclosing frame
                    if let Some(target) = self.enclosing_alias_target(&name) {
                        return Ok(self.compile_name_place(&target));
                    }
                }
                Ok(None)
            }
            Expr::Path(p) if p.path.segments.len() == 1 && p.qself.is_none() => {
                let name = self.unalias(&p.path.segments[0].ident.to_string());
                Ok(self.compile_name_place(&name))
            }
            Expr::Field(f) => {
                let (base, parent) = self.compile_base_with_back(&f.base)?;
                let member = self.member_of(&f.member);
                let dst = self.alloc();
                self.emit(Op::UniqueField { dst, base, member });
                Ok(Some(Place {
                    reg: dst,
                    back: PlaceBack::Field {
                        base,
                        member,
                        parent: Box::new(parent),
                    },
                }))
            }
            Expr::Index(ix) => {
                let (base, parent) = self.compile_base_with_back(&ix.expr)?;
                let key = self.compile_expr(&ix.index)?;
                let dst = self.alloc();
                self.emit(Op::UniqueIndex { dst, base, key });
                Ok(Some(Place {
                    reg: dst,
                    back: PlaceBack::Index {
                        base,
                        key,
                        parent: Box::new(parent),
                    },
                }))
            }
            _ => Ok(None),
        }
    }

    fn compile_name_place(&mut self, name: &str) -> Option<Place> {
        match self.resolve_for_write(name) {
            NameLoc::Local(reg) => {
                // the caller split a borrow parameter before the call
                if !self.cur().borrow_params.contains(&reg) {
                    self.emit(Op::UniqueReg { reg });
                }
                Some(Place {
                    reg,
                    back: PlaceBack::None,
                })
            }
            NameLoc::Cell(cell) => {
                let dst = self.alloc();
                self.emit(Op::UniqueCell { dst, cell });
                Some(Place {
                    reg: dst,
                    back: PlaceBack::Cell(cell),
                })
            }
            NameLoc::Upvalue(idx) => {
                let dst = self.alloc();
                self.emit(Op::UniqueUpvalue { dst, idx });
                Some(Place {
                    reg: dst,
                    back: PlaceBack::Upvalue(idx),
                })
            }
            NameLoc::None => None,
        }
    }

    /// A temporary base may still share storage with what produced it, so it is split too.
    pub(super) fn compile_place_base(&mut self, expr: &Expr) -> Result<Reg> {
        Ok(self.compile_base_with_back(expr)?.0)
    }

    /// `compile_place_base` plus how the base stores back.
    fn compile_base_with_back(&mut self, expr: &Expr) -> Result<(Reg, PlaceBack)> {
        if let Some(place) = self.compile_place(expr)? {
            return Ok((place.reg, place.back));
        }
        let reg = self.compile_expr(expr)?;
        self.emit(Op::UniqueReg { reg });
        Ok((reg, PlaceBack::None))
    }

    pub(super) fn emit_place_writeback(&mut self, place: &Place) {
        self.emit_back(place.reg, &place.back);
    }

    /// 1 level, then the base's own. A string projection replaces its base buffer and only the
    /// parent chain lands that.
    fn emit_back(&mut self, reg: Reg, back: &PlaceBack) {
        match back {
            PlaceBack::None => {}
            PlaceBack::Cell(cell) => self.emit(Op::StoreCell {
                cell: *cell,
                src: reg,
            }),
            PlaceBack::Upvalue(idx) => self.emit(Op::StoreUpvalue {
                idx: *idx,
                src: reg,
            }),
            PlaceBack::Field {
                base,
                member,
                parent,
            } => {
                self.emit(Op::SetField {
                    base: *base,
                    member: *member,
                    val: reg,
                });
                self.emit_back(*base, parent);
            }
            PlaceBack::Index { base, key, parent } => {
                self.emit(Op::SetIndex {
                    base: *base,
                    key: *key,
                    val: reg,
                });
                self.emit_back(*base, parent);
            }
        }
    }

    /// `let r = &mut PLACE`. A variable borrow becomes a name alias, a field or element borrow a
    /// real reference value.
    pub(super) fn compile_let_borrow(
        &mut self,
        local: &syn::Local,
        dst: Reg,
        is_last: bool,
    ) -> Result<bool> {
        let Some(init) = &local.init else {
            return Ok(false);
        };
        if init.diverge.is_some() {
            return Ok(false);
        }
        let name = match &local.pat {
            syn::Pat::Ident(id) if id.subpat.is_none() => id.ident.to_string(),
            syn::Pat::Type(t) => match &*t.pat {
                syn::Pat::Ident(id) if id.subpat.is_none() => id.ident.to_string(),
                _ => return Ok(false),
            },
            _ => return Ok(false),
        };
        let Expr::Reference(r) = &*init.expr else {
            return Ok(false);
        };
        if r.mutability.is_none() {
            return Ok(false);
        }
        if let Some(var) = single_path_name(&r.expr) {
            let target = self.unalias(&var);
            if matches!(self.resolve(&target), NameLoc::None) {
                return Ok(false);
            }
            self.cur().aliases.insert(name, target);
            if is_last {
                self.emit(Op::LoadUnit { dst });
            }
            return Ok(true);
        }
        if !matches!(&*r.expr, Expr::Field(_) | Expr::Index(_)) {
            return Ok(false);
        }
        let Some(place) = self.compile_place(&r.expr)? else {
            return Ok(false);
        };
        let reg = self.alloc();
        match place.back {
            PlaceBack::Index { base, key, .. } => self.emit(Op::RefIndex {
                dst: reg,
                base,
                key,
            }),
            PlaceBack::Field { base, member, .. } => self.emit(Op::RefField {
                dst: reg,
                base,
                member,
            }),
            _ => anyhow::bail!("projection borrow without a projection place"),
        }
        self.define(&name, reg);
        if is_last {
            self.emit(Op::LoadUnit { dst });
        }
        Ok(true)
    }

    /// A `&mut place` scrutinee wraps the place as a borrow, so bindings write through.
    pub(super) fn compile_scrutinee(&mut self, expr: &Expr) -> Result<Reg> {
        if let Expr::Reference(r) = expr
            && r.mutability.is_some()
        {
            let place = self.compile_mut_receiver(&r.expr)?;
            let dst = self.alloc();
            self.emit(Op::MakeBorrow {
                dst,
                src: place.reg,
            });
            return Ok(dst);
        }
        self.compile_expr(expr)
    }

    /// The place, or the plain expression split from sharing.
    pub(super) fn compile_mut_receiver(&mut self, expr: &Expr) -> Result<Place> {
        if let Some(place) = self.compile_place(expr)? {
            return Ok(place);
        }
        let reg = self.compile_expr(expr)?;
        self.emit(Op::UniqueReg { reg });
        Ok(Place {
            reg,
            back: PlaceBack::None,
        })
    }
}

impl Compiler<'_> {
    /// `mem::swap`, `mem::take` and `mem::replace` replace whole values, so nothing is mutated in
    /// place. False hands the call back.
    pub(super) fn compile_mem_intrinsic(
        &mut self,
        dst: Reg,
        kind: PathId,
        c: &syn::ExprCall,
    ) -> Result<bool> {
        let strip = |e: &Expr| match e {
            Expr::Reference(r) if r.mutability.is_some() => Some(r.expr.clone()),
            _ => None,
        };
        match kind {
            PathId::MemSwap if c.args.len() == 2 => {
                let (Some(a), Some(b)) = (strip(&c.args[0]), strip(&c.args[1])) else {
                    return Ok(false);
                };
                let (Some(pa), Some(pb)) = (self.compile_place(&a)?, self.compile_place(&b)?)
                else {
                    return Ok(false);
                };
                let tmp = self.alloc();
                self.emit(Op::Move {
                    dst: tmp,
                    src: pa.reg,
                });
                self.emit(Op::Move {
                    dst: pa.reg,
                    src: pb.reg,
                });
                self.emit(Op::Move {
                    dst: pb.reg,
                    src: tmp,
                });
                self.emit_place_writeback(&pa);
                self.emit_place_writeback(&pb);
                self.emit(Op::LoadUnit { dst });
                Ok(true)
            }
            PathId::MemTake if c.args.len() == 1 => {
                let Some(a) = strip(&c.args[0]) else {
                    return Ok(false);
                };
                let Some(pa) = self.compile_place(&a)? else {
                    return Ok(false);
                };
                let old = self.alloc();
                self.emit(Op::Move {
                    dst: old,
                    src: pa.reg,
                });
                self.emit(Op::DefaultOf {
                    dst: pa.reg,
                    src: old,
                });
                self.emit_place_writeback(&pa);
                self.emit(Op::Move { dst, src: old });
                Ok(true)
            }
            PathId::MemReplace if c.args.len() == 2 => {
                let Some(a) = strip(&c.args[0]) else {
                    return Ok(false);
                };
                let Some(pa) = self.compile_place(&a)? else {
                    return Ok(false);
                };
                let new = self.compile_expr(&c.args[1])?;
                let old = self.alloc();
                self.emit(Op::Move {
                    dst: old,
                    src: pa.reg,
                });
                self.emit(Op::Move {
                    dst: pa.reg,
                    src: new,
                });
                self.emit_place_writeback(&pa);
                self.emit(Op::Move { dst, src: old });
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// A name, a field, an element or a deref, seen through parens.
pub(super) fn is_place_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(p) => is_place_expr(&p.expr),
        Expr::Group(g) => is_place_expr(&g.expr),
        Expr::Field(_) | Expr::Index(_) => true,
        Expr::Unary(u) => matches!(u.op, syn::UnOp::Deref(_)),
        other => single_path_name(other).is_some(),
    }
}

pub(super) fn single_path_name(expr: &Expr) -> Option<String> {
    if let Expr::Path(p) = expr
        && p.path.segments.len() == 1
        && p.qself.is_none()
    {
        return Some(p.path.segments[0].ident.to_string());
    }
    None
}
