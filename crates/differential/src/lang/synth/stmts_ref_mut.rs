//! Generation of the statements that take a `&mut` into a binding and write through it.

use rand::RngExt;

use crate::lang::stmt::{ElemWrite, MutPlace, Stmt};
use crate::lang::synth::Generator;
use crate::lang::ty::Ty;

/// A `MutPlace` before its expressions are built.
#[derive(Clone)]
enum Form {
    Whole,
    Field(usize),
    Index,
    GetMut,
    LastMut,
    MapGetMut,
    Entry,
    ValuesMut,
    OptRefMut,
}

/// Every form that reaches inside a binding of type `base`, with the type behind the `&mut`.
fn inner_forms(base: &Ty) -> Vec<(Form, Ty)> {
    match base {
        Ty::User(shape) => shape
            .fields()
            .iter()
            .enumerate()
            .map(|(index, field)| (Form::Field(index), field.ty.clone()))
            .collect(),
        Ty::Tuple(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| (Form::Field(index), item.clone()))
            .collect(),
        Ty::Vec(elem) => [Form::Index, Form::GetMut, Form::LastMut]
            .into_iter()
            .map(|form| (form, (**elem).clone()))
            .collect(),
        Ty::Map(_, value) => [Form::MapGetMut, Form::Entry, Form::ValuesMut]
            .into_iter()
            .map(|form| (form, (**value).clone()))
            .collect(),
        Ty::Opt(inner) => vec![(Form::OptRefMut, (**inner).clone())],
        _ => Vec::new(),
    }
}

impl Generator<'_> {
    /// `{ let r = &mut x; .. }` and the other forms of `MutPlace`, on a binding nothing holds.
    pub(super) fn ref_mut_stmt(&mut self) -> Option<Stmt> {
        let (name, base) = self.pick_writable()?;
        let inner = inner_forms(&base);
        let (form, elem) = if !inner.is_empty() && self.chance(0.8) {
            self.pick(&inner).clone()
        } else {
            (Form::Whole, base.clone())
        };
        let var = self.fresh("diff_e");
        // the binding is borrowed for the whole statement, so its name is hidden
        let (place, write) = self.without_binding(&name, |inner| {
            let place = inner.mut_place(&form, &base);
            let write = inner.write_through(&place, &var, &elem);
            (place, write)
        });
        Some(Stmt::RefMut {
            name,
            base,
            place,
            var,
            elem,
            write,
        })
    }

    fn mut_place(&mut self, form: &Form, base: &Ty) -> MutPlace {
        let key_val = base
            .key_val()
            .map(|(key, value)| (key.clone(), value.clone()));
        match (form, key_val) {
            (Form::Whole, _) => MutPlace::Whole,
            (Form::Field(index), _) => MutPlace::Field(*index),
            (Form::Index, _) => MutPlace::Index(self.rng.random_range(0..=3)),
            (Form::GetMut, _) => MutPlace::GetMut(self.rng.random_range(0..=3)),
            (Form::LastMut, _) => MutPlace::LastMut,
            (Form::ValuesMut, _) => MutPlace::ValuesMut,
            (Form::OptRefMut, _) => MutPlace::OptRefMut,
            (Form::MapGetMut, Some((key, _))) => MutPlace::MapGetMut(self.expr(&key, 1)),
            (Form::Entry, Some((key, value))) => {
                let key = self.expr(&key, 1);
                let default = self.closure_body(|inner| inner.expr(&value, 1));
                MutPlace::Entry { key, default }
            }
            (Form::MapGetMut | Form::Entry, None) => {
                unreachable!("inner_forms offers the map forms on a map only")
            }
        }
    }

    /// The write, with the clone of the old value in scope as `var`.
    fn write_through(&mut self, place: &MutPlace, var: &str, elem: &Ty) -> ElemWrite {
        if matches!(place, MutPlace::ValuesMut) {
            return self.entry_write(var, elem);
        }
        let build = |inner: &mut Self| {
            inner.push_local(var.to_string(), elem.clone());
            inner.elem_write(elem)
        };
        if place.is_conditional() {
            self.begin_branches();
            let write = self.branch(build);
            self.end_branches();
            write
        } else {
            self.scoped(build)
        }
    }

    /// The write of one map entry. It sees the entry alone and can neither panic nor print a
    /// drop, a different entry order would show in both.
    fn entry_write(&mut self, var: &str, elem: &Ty) -> ElemWrite {
        let locals = [(var.to_string(), elem.clone())];
        for _ in 0..4 {
            let write = self.closure_body(|inner| {
                inner.without_scope(|inner| {
                    inner.with_locals(&locals, |inner| inner.elem_write(elem))
                })
            });
            let traced = write.exprs().iter().any(|expr| expr.mentions_trace());
            if !write.has_fallible_op() && !traced {
                return write;
            }
        }
        ElemWrite::Assign(self.literal(elem))
    }
}
