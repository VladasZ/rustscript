mod anyhow_bridge;
mod assoc;
mod borrow;
mod bridge;
mod bytecode;
mod cell;
mod chrono_bridge;
mod compile;
mod console;
pub mod coverage;
mod crates_bridge;
mod debug_fmt;
mod discard;
mod ed25519_bridge;
mod enum_def;
mod env_overlay;
mod format;
mod higher_order;
mod http;
mod impls;
mod int_methods;
mod iterator;
mod json_bridge;
mod json_paths;
mod json_scalar;
mod jwt_bridge;
mod map_methods;
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
mod register;
mod resolver;
mod rs_str;
mod serde_attrs;
mod serde_types;
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
use register::{
    PendingConst, build_fn_index, build_impl_table, build_module_tree, collect_const_types,
    collect_fn_signatures, collect_impl_items, collect_mut_methods, collect_traits,
    impl_name_tables, register_items, returns_anyhow_result,
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
    /// `#[serde(default)]` fields by struct and slot, each a chunk that makes the value
    serde_defaults: SerdeDefaults,
    /// for the bridge dispatch to expand aliases
    main_index: Option<u32>,
    /// `main` returns an `anyhow::Result`
    main_err_display: bool,
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

        // every chunk compiles against the same tables, only its module and impl differ
        let base = Ctx {
            resolver: &resolver,
            module: 0,
            file: modules[0].file.clone(),
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
        let mut functions = Vec::with_capacity(pending_fns.len());
        for (m, f) in &pending_fns {
            let ctx = base.at(*m, modules, None);
            let mut c = Compiler::new(&ctx);
            functions.push(Arc::new(c.compile_fn(&f.sig, &f.block)?));
        }
        let mut methods = Vec::with_capacity(pending_methods.len());
        for (ty, name, m, f) in &pending_methods {
            let ctx = base.at(*m, modules, Some(ty));
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
            let ctx = base.at(*m, modules, None);
            let mut c = Compiler::new(&ctx);
            globals.push(GlobalSlot::Todo(Arc::new(c.compile_const(expr, ty)?)));
        }
        let serde_defaults = compile_serde_defaults(&base, modules)?;

        let fn_index = build_fn_index(&resolver);
        let main_index = resolver.modules[0].fns.get("main").copied();
        let uses = resolver.modules[0].uses.clone();
        let main_err_display = main_index
            .and_then(|i| pending_fns.get(i as usize))
            .is_some_and(|(_, f)| returns_anyhow_result(&f.sig.output, &uses));
        Ok(Interp {
            functions,
            fn_index,
            impls,
            resolver,
            globals: RefCell::new(globals),
            serde_defaults,
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
            .thread_stack_size(vm::SCRIPT_STACK_BYTES)
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
            serde_enums: self.build_enums(),
            enums,
            unit_structs,
            struct_names,
            has_drop: self.impls.any_drop(),
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
            .stack_size(vm::SCRIPT_STACK_BYTES)
            .spawn(move || runner.run_chunk(&main_chunk, &[], &[], true))
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
            // a compiled binary prints `Debug` here, an `anyhow::Result` main holds an anyhow
            // error even when the script returned a bare one through `into`
            let render = |v: &value::Value| {
                if self.main_err_display {
                    anyhow_bridge::into_anyhow(v.clone()).debug()
                } else {
                    v.debug()
                }
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

/// The chunk of each `#[serde(default)]` field, by struct and slot.
type SerdeDefaults = HashMap<(Arc<str>, usize), Arc<Chunk>>;

/// One chunk per `#[serde(default)]` field, compiled in the struct's module.
fn compile_serde_defaults(base: &Ctx<'_>, modules: &[ModuleSrc]) -> Result<SerdeDefaults> {
    let mut out = HashMap::new();
    for (canon, def) in &base.resolver.structs {
        let syn::Fields::Named(named) = &def.ast.fields else {
            continue;
        };
        // the slot counts named fields like `build_structs`
        for (slot, field) in named.named.iter().enumerate() {
            let Some(default) = serde_attrs::serde_default(field) else {
                continue;
            };
            let expr = serde_default_expr(&default, &def.ast.ident)?;
            let ctx = base.at(def.module, modules, Some(canon));
            let chunk = Compiler::new(&ctx).compile_const(&expr, &field.ty)?;
            out.insert((canon.clone(), slot), Arc::new(chunk));
        }
    }
    Ok(out)
}

/// The expression a missing `#[serde(default)]` field evaluates. A path may name `Self`, which
/// is the struct.
fn serde_default_expr(
    default: &serde_attrs::SerdeDefault,
    owner: &syn::Ident,
) -> Result<syn::Expr> {
    Ok(match default {
        serde_attrs::SerdeDefault::Type => syn::parse_quote!(Default::default()),
        serde_attrs::SerdeDefault::Path(text) => {
            let mut path: syn::Path = syn::parse_str(text)
                .map_err(|e| anyhow!("bad `#[serde(default = \"{text}\")]`: {e}"))?;
            if let Some(first) = path.segments.first_mut()
                && first.ident == "Self"
            {
                first.ident = owner.clone();
            }
            syn::parse_quote!(#path())
        }
    })
}
