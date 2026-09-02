//! User `Display`, `Debug` and `Drop` impls run through the VM.

use std::mem::take;
use std::sync::Arc;

use anyhow::Result;

use crate::interpreter::bytecode::Chunk;
use crate::interpreter::native::Native;
use crate::interpreter::value::Value;
use crate::interpreter::vm::Vm;

use super::template::render_template;

impl Vm {
    // format

    pub(crate) fn render_fmt(
        self: &Arc<Self>,
        chunk: &Chunk,
        spec: u16,
        regs: &[Value],
    ) -> Result<String> {
        let f = &chunk.fmts[spec as usize];
        let positional: Vec<Value> = f
            .positional
            .iter()
            .map(|r| regs[*r as usize].clone())
            .collect();
        let named: Vec<(&str, Value)> = f
            .named
            .iter()
            .map(|(n, r)| (n.as_str(), regs[*r as usize].clone()))
            .collect();
        render_template(self, &f.template, &positional, &named)
    }

    /// None when the type has no user `Display` or `Debug` impl.
    pub(crate) fn user_fmt_text(
        self: &Arc<Self>,
        v: &Value,
        debug: bool,
    ) -> Result<Option<String>> {
        Ok(self.user_fmt(v, debug)?.map(|(text, _)| text))
    }

    /// `user_fmt_text` plus whether the impl padded through `f.pad`.
    pub(crate) fn user_fmt(
        self: &Arc<Self>,
        v: &Value,
        debug: bool,
    ) -> Result<Option<(String, bool)>> {
        let Some(methods) = self.impls.of_value(v) else {
            return Ok(None);
        };
        let Some(chunk) = (if debug {
            &methods.debug
        } else {
            &methods.display
        })
        .clone() else {
            return Ok(None);
        };
        let handle = Arc::new(parking_lot::Mutex::new(Native::Fmt {
            text: String::new(),
            padded: false,
        }));
        let args = vec![v.clone(), Value::Native(handle.clone())];
        self.run_chunk(&chunk, &args, &[])?;
        let out = match &*handle.lock() {
            Native::Fmt { text, padded } => (text.clone(), *padded),
            _ => (String::new(), false),
        };
        Ok(Some(out))
    }

    /// Drops a value the current frame owns. A moved binding was cleared by its `Take`, so a
    /// value that is still here is owned here. Containers hand their contents on. `Rc` and `Arc`
    /// are real shared handles, the last one drops the content, so a cycle leaks like in real
    /// Rust.
    pub(crate) fn run_user_drop(self: &Arc<Self>, value: Value) -> Result<()> {
        match value {
            Value::Struct(s) => {
                self.run_drop_impl(Value::Struct(s.clone()))?;
                // fields drop after `Drop::drop` in declaration order
                let fields = take(&mut *s.values.lock());
                for field in fields {
                    self.run_user_drop(field)?;
                }
                Ok(())
            }
            Value::Enum { def, variant, data } => {
                self.run_drop_impl(Value::Enum {
                    def,
                    variant,
                    data: data.clone(),
                })?;
                let payload = take(&mut *data.lock());
                for field in payload {
                    self.run_user_drop(field)?;
                }
                Ok(())
            }
            Value::Vec(list) | Value::Tuple(list) => {
                let items = take(&mut *list.lock());
                for item in items {
                    self.run_user_drop(item)?;
                }
                Ok(())
            }
            Value::Map(map, _) => {
                let entries = take(&mut *map.lock());
                for (_, entry) in entries {
                    self.run_user_drop(entry)?;
                }
                Ok(())
            }
            Value::Cell(kind, slot) => {
                if kind.is_shared_pointer() && Arc::strong_count(&slot) != 1 {
                    return Ok(());
                }
                let inner = take(&mut *slot.lock());
                self.run_user_drop(inner)
            }
            Value::Native(handle) => {
                let leftover = match &mut *handle.lock() {
                    Native::Iterator(state) => state.take_remaining(),
                    _ => Vec::new(),
                };
                for item in leftover {
                    self.run_user_drop(item)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn run_drop_impl(self: &Arc<Self>, value: Value) -> Result<()> {
        let Some(chunk) = self
            .impls
            .of_value(&value)
            .and_then(|methods| methods.drop.clone())
        else {
            return Ok(());
        };
        self.run_chunk(&chunk, &[value], &[])?;
        Ok(())
    }

    // path values
}
