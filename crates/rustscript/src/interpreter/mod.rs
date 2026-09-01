mod assoc;
mod borrow;
mod bridge;
mod bytecode;
mod cell;
mod compile;
mod console;
pub mod coverage;
mod crates_bridge;
mod debug_fmt;
mod ed25519_bridge;
mod enum_def;
mod format;
mod higher_order;
mod http;
mod impls;
mod int_methods;
mod items;
mod iterator;
mod json_bridge;
mod jwt_bridge;
mod methods;
mod native;
mod native_methods;
mod numeric;
mod ops;
mod pattern;
mod pdf_bridge;
mod process;
mod ratatui;
mod ratatui_bridge;
mod ratatui_render;
mod regex_bridge;
mod resolver;
mod rs_str;
mod serde_attrs;
mod service_bridge;
mod shared;
mod std_bridge;
mod typeir;
mod value;
mod vecmap;
mod vm;
mod vm_method;
mod vm_step;
mod vm_support;
mod winreg_bridge;
mod wmi_bridge;
mod xmltree_bridge;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow};

use crate::loader::ModuleSrc;
use bytecode::Chunk;
use compile::{Compiler, Ctx};
use items::{
    PendingConst, build_impl_table, build_module_tree, collect_const_types, collect_impl_items,
    collect_mut_methods, collect_traits, impl_name_tables, register_item,
};
use resolver::{Resolver, StructDef};
pub use vm_support::{ErrReturn, ScriptPanic};

/// Set by the real Ctrl-C handler, drained between loop iterations to run the script's own handler.
static CTRLC_HIT: AtomicBool = AtomicBool::new(false);
static CTRLC_INSTALLED: OnceLock<bool> = OnceLock::new();
static CTRLC_HANDLER: parking_lot::Mutex<Option<value::Value>> = parking_lot::Mutex::new(None);

pub(crate) fn set_ctrlc_handler(closure: value::Value) -> Result<()> {
    *CTRLC_HANDLER.lock() = Some(closure);
    if CTRLC_INSTALLED.set(true).is_ok() {
        ctrlc::set_handler(|| CTRLC_HIT.store(true, Ordering::SeqCst))
            .map_err(|e| anyhow!("could not install ctrl-c handler: {e}"))?;
    }
    Ok(())
}

/// Drains the flag, so each Ctrl-C runs the handler once.
pub(crate) fn pending_ctrlc_handler() -> Option<value::Value> {
    // relaxed load first, this runs on every loop iteration
    if !CTRLC_HIT.load(Ordering::Relaxed) {
        return None;
    }
    if !CTRLC_HIT.swap(false, Ordering::SeqCst) {
        return None;
    }
    CTRLC_HANDLER.lock().clone()
}

/// Index 0 is the script path, like in a real binary.
static SCRIPT_ARGS: OnceLock<Vec<String>> = OnceLock::new();

pub fn set_script_args(args: Vec<String>) {
    SCRIPT_ARGS
        .set(args)
        .expect("script args are set exactly once");
}

pub(crate) fn script_args() -> Vec<String> {
    SCRIPT_ARGS.get().cloned().unwrap_or_default()
}

/// `async_mode` means the script has `#[tokio::main]`.
pub fn run(modules: &[ModuleSrc], async_mode: bool) -> Result<()> {
    let interp = Interp::load(modules, async_mode)?;
    // Coverage walk first. Otherwise an unchecked script can die on a cold branch after doing
    // half of its side effects. Costs well under a millisecond.
    interp.coverage_gate()?;
    interp.run()
}

/// Compiled once, evaluated on first read.
enum GlobalSlot {
    Todo(Arc<Chunk>),
}

pub struct Interp {
    /// indexed by id, direct calls use the id
    functions: Vec<Arc<Chunk>>,
    /// for calls resolved at runtime
    fn_index: HashMap<String, u32>,
    impls: Arc<impls::ImplTable>,
    resolver: Resolver,
    /// lazy, so declaration order doesn't matter
    globals: RefCell<Vec<GlobalSlot>>,
    /// for the bridge dispatch to expand aliases
    main_index: Option<u32>,
    /// an `Err` out of `main` prints `Display` instead of `Debug`
    main_err_display: bool,
}

/// Real Rust prints the `Debug` form, except `anyhow::Error` whose `Debug` is the bare message.
/// A `Result` with less than 2 type arguments is the anyhow shape unless the imports say otherwise.
fn main_err_uses_display(output: &syn::ReturnType, uses: &HashMap<String, Vec<String>>) -> bool {
    let from_anyhow = |segs: &[String]| -> bool {
        match segs {
            [one] => uses
                .get(one)
                .is_some_and(|full| full.first().is_some_and(|s| s == "anyhow")),
            [first, ..] => first == "anyhow",
            [] => false,
        }
    };
    let syn::ReturnType::Type(_, ty) = output else {
        return false;
    };
    let syn::Type::Path(p) = &**ty else {
        return false;
    };
    let Some(last) = p.path.segments.last() else {
        return false;
    };
    if last.ident != "Result" {
        return false;
    }
    let mut types = Vec::new();
    if let syn::PathArguments::AngleBracketed(ab) = &last.arguments {
        for a in &ab.args {
            if let syn::GenericArgument::Type(t) = a {
                types.push(t);
            }
        }
    }
    let Some(err_ty) = types.get(1) else {
        // a plain `Result<()>` resolves through the imports
        let segs: Vec<String> = p
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        return from_anyhow(&segs);
    };
    let syn::Type::Path(ep) = err_ty else {
        return false;
    };
    let segs: Vec<String> = ep
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    from_anyhow(&segs)
}

/// Return type of every function declared exactly once. A name defined twice with different
/// return types is skipped, the call site can't tell which one it hits.
fn register_items(
    resolver: &mut Resolver,
    modules: &[ModuleSrc],
    pending_fns: &mut Vec<(usize, Rc<syn::ItemFn>)>,
    pending_impls: &mut Vec<(usize, Rc<syn::ItemImpl>)>,
    pending_consts: &mut Vec<PendingConst>,
) -> Result<()> {
    for (m, src) in modules.iter().enumerate() {
        for item in &src.items {
            register_item(
                resolver,
                m,
                item,
                pending_fns,
                pending_impls,
                pending_consts,
            )?;
        }
    }
    Ok(())
}

/// So a call to a generic helper can read the type its arguments give to a type parameter.
fn collect_fn_signatures(
    pending_fns: &[(usize, Rc<syn::ItemFn>)],
) -> HashMap<String, syn::Signature> {
    let mut seen: HashMap<String, Option<syn::Signature>> = HashMap::default();
    for (_, f) in pending_fns {
        seen.entry(f.sig.ident.to_string())
            .and_modify(|known| *known = None)
            .or_insert_with(|| Some(f.sig.clone()));
    }
    seen.into_iter()
        .filter_map(|(name, sig)| sig.map(|sig| (name, sig)))
        .collect()
}
fn build_fn_index(resolver: &Resolver) -> HashMap<String, u32> {
    let mut fn_index = HashMap::default();
    for syms in &resolver.modules {
        let prefix = syms.path.join("::");
        for (name, &idx) in &syms.fns {
            let key = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}::{name}")
            };
            fn_index.insert(key, idx);
        }
    }
    fn_index
}

impl Interp {
    pub fn load(modules: &[ModuleSrc], async_mode: bool) -> Result<Self> {
        let mut resolver = build_module_tree(modules);
        let mut pending_fns: Vec<(usize, Rc<syn::ItemFn>)> = Vec::new();
        let mut pending_impls: Vec<(usize, Rc<syn::ItemImpl>)> = Vec::new();
        let mut pending_consts: Vec<PendingConst> = Vec::new();

        let traits = collect_traits(modules);
        register_items(
            &mut resolver,
            modules,
            &mut pending_fns,
            &mut pending_impls,
            &mut pending_consts,
        )?;
        resolver.reject_module_globs()?;

        let pending_methods =
            collect_impl_items(&mut resolver, &pending_impls, &traits, &mut pending_consts)?;

        let fn_signatures = collect_fn_signatures(&pending_fns);

        let (has_drop, mut_methods) = collect_mut_methods(&pending_methods);
        let (impl_methods, method_atoms) = impl_name_tables(&pending_methods);
        let impl_sigs: HashMap<(String, String), syn::Signature> = pending_methods
            .iter()
            .map(|(ty, name, _, f)| ((ty.clone(), name.clone()), f.sig.clone()))
            .collect();
        let const_types = collect_const_types(modules, &resolver);

        let mut functions = Vec::with_capacity(pending_fns.len());
        for (m, f) in &pending_fns {
            let ctx = Ctx {
                resolver: &resolver,
                module: *m,
                file: modules[*m].file.clone(),
                async_mode,
                impl_type: None,
                fn_signatures: &fn_signatures,
                mut_methods: &mut_methods,
                impl_methods: &impl_methods,
                method_atoms: &method_atoms,
                impl_sigs: &impl_sigs,
                const_types: &const_types,
                has_drop,
            };
            let mut c = Compiler::new(&ctx);
            functions.push(Arc::new(c.compile_fn(&f.sig, &f.block)?));
        }
        let mut methods = Vec::with_capacity(pending_methods.len());
        for (ty, name, m, f) in &pending_methods {
            let ctx = Ctx {
                resolver: &resolver,
                module: *m,
                file: modules[*m].file.clone(),
                async_mode,
                impl_type: Some(ty),
                fn_signatures: &fn_signatures,
                mut_methods: &mut_methods,
                impl_methods: &impl_methods,
                method_atoms: &method_atoms,
                impl_sigs: &impl_sigs,
                const_types: &const_types,
                has_drop,
            };
            let mut c = Compiler::new(&ctx);
            methods.push((
                ty.clone(),
                name.clone(),
                Arc::new(c.compile_fn(&f.sig, &f.block)?),
            ));
        }
        let impls = build_impl_table(&resolver, methods, &method_atoms);
        let mut globals = Vec::with_capacity(pending_consts.len());
        for (m, expr, ty) in &pending_consts {
            let ctx = Ctx {
                resolver: &resolver,
                module: *m,
                file: modules[*m].file.clone(),
                async_mode,
                impl_type: None,
                fn_signatures: &fn_signatures,
                mut_methods: &mut_methods,
                impl_methods: &impl_methods,
                method_atoms: &method_atoms,
                impl_sigs: &impl_sigs,
                const_types: &const_types,
                has_drop,
            };
            let mut c = Compiler::new(&ctx);
            globals.push(GlobalSlot::Todo(Arc::new(c.compile_const(expr, ty)?)));
        }

        let fn_index = build_fn_index(&resolver);
        let main_index = resolver.modules[0].fns.get("main").copied();
        let uses = resolver.modules[0].uses.clone();
        let main_err_display = main_index
            .and_then(|i| pending_fns.get(i as usize))
            .is_some_and(|(_, f)| main_err_uses_display(&f.sig.output, &uses));
        Ok(Interp {
            functions,
            fn_index,
            impls,
            resolver,
            globals: RefCell::new(globals),
            main_index,
            main_err_display,
        })
    }

    /// used by `rust check`
    pub fn coverage(&self) -> Vec<coverage::Finding> {
        coverage::report(&self.functions, self.impls.names())
    }

    /// `rust check` and every interpreted run share this report
    pub fn coverage_gate(&self) -> Result<()> {
        let findings = self.coverage();
        if findings.is_empty() {
            return Ok(());
        }
        let mut out = String::new();
        for finding in &findings {
            out.push_str("  ");
            out.push_str(&finding.message());
            out.push('\n');
        }
        let (count, verb) = if findings.len() == 1 {
            ("1 method".to_string(), "is")
        } else {
            (format!("{} methods", findings.len()), "are")
        };
        Err(anyhow!(
            "{count} used by this script {verb} not implemented by the interpreter:\n{}",
            out.trim_end()
        ))
    }

    fn run(&self) -> Result<()> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow!("cannot start tokio runtime: {e}"))?;
        let functions = self.functions.clone();
        let globals: Vec<parking_lot::Mutex<vm::GlobalSlot>> = self
            .globals
            .borrow()
            .iter()
            .map(|slot| {
                let GlobalSlot::Todo(c) = slot;
                parking_lot::Mutex::new(vm::GlobalSlot::Todo(c.clone()))
            })
            .collect();
        // precomputed, nothing at runtime may touch the syn AST, it is not `Send`
        let enums: Vec<Arc<enum_def::EnumDef>> =
            self.resolver.enum_defs.values().cloned().collect();
        let unit_structs: Vec<Arc<str>> = self
            .resolver
            .structs
            .iter()
            .filter(|(_, def)| matches!(def.ast.fields, syn::Fields::Unit))
            .map(|(name, _)| Arc::from(&**name))
            .collect();
        let struct_names: std::collections::HashSet<String> = self
            .resolver
            .structs
            .keys()
            .map(ToString::to_string)
            .collect();
        let pinterp = Arc::new(vm::Vm {
            functions,
            fn_index: self.fn_index.clone(),
            impls: self.impls.clone(),
            globals,
            structs: self.build_structs(),
            enums,
            unit_structs,
            struct_names,
            rt: rt.handle().clone(),
        });
        let idx = self
            .main_index
            .ok_or_else(|| anyhow!("no `main` function found"))? as usize;
        let main_chunk = pinterp.functions[idx].clone();
        let runner = pinterp.clone();
        // A plain thread, not a blocking task. So `main` takes no tokio task id and the first
        // `tokio::spawn` gets the same id as in a compiled binary.
        let joined = std::thread::Builder::new()
            .name("main".to_string())
            .spawn(move || runner.run_chunk(&main_chunk, &[], &[]))
            .map_err(|e| anyhow!("cannot start main thread: {e}"))?
            .join();
        let ret = joined.map_err(|payload| {
            let msg = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(ToString::to_string))
                .unwrap_or_else(|| "unknown panic".to_string());
            anyhow!("main task panicked: {msg}")
        })??;
        if let value::Value::Enum { def, variant, data } = &ret
            && def.kind == enum_def::EnumKind::Result
            && *variant == enum_def::ERR
        {
            // a compiled binary prints `Debug` here, anyhow prints the bare message
            let render: fn(&value::Value) -> String = if self.main_err_display {
                value::Value::display
            } else {
                value::Value::debug
            };
            let msg = data.lock().first().map(render).unwrap_or_default();
            return Err(anyhow::Error::new(vm_support::ErrReturn(msg)));
        }
        Ok(())
    }

    fn structs(&self) -> &HashMap<Arc<str>, StructDef> {
        &self.resolver.structs
    }

    fn resolver(&self) -> &Resolver {
        &self.resolver
    }
}
