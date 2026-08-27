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
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow, bail};
use quote::ToTokens;
use syn::Item;

use crate::loader::ModuleSrc;
use bytecode::Chunk;
use compile::{Compiler, Ctx};
use resolver::{ModuleSyms, Res, Resolver, StructDef};
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

fn build_module_tree(modules: &[ModuleSrc]) -> Resolver {
    let index: HashMap<String, usize> = modules
        .iter()
        .enumerate()
        .map(|(i, m)| (m.path.join("::"), i))
        .collect();
    let mut syms: Vec<ModuleSyms> = modules
        .iter()
        .map(|m| ModuleSyms {
            path: m.path.clone(),
            crate_root: m.crate_root,
            ..ModuleSyms::default()
        })
        .collect();
    for (i, m) in modules.iter().enumerate() {
        if let Some((name, parent_path)) = m.path.split_last() {
            let parent = index[&parent_path.join("::")];
            syms[i].parent = Some(parent);
            syms[parent].children.insert(name.clone(), i);
        }
    }
    Resolver {
        modules: syms,
        structs: HashMap::default(),
        enums: HashMap::default(),
        enum_defs: HashMap::default(),
        type_ids: HashMap::default(),
    }
}

fn register_item(
    resolver: &mut Resolver,
    m: usize,
    item: &Item,
    pending_fns: &mut Vec<(usize, Rc<syn::ItemFn>)>,
    pending_impls: &mut Vec<(usize, Rc<syn::ItemImpl>)>,
    pending_consts: &mut Vec<PendingConst>,
) -> Result<()> {
    match item {
        Item::Fn(f) => {
            let name = f.sig.ident.to_string();
            resolver.modules[m].fns.insert(
                name,
                u32::try_from(pending_fns.len()).expect("table fits u32"),
            );
            pending_fns.push((m, Rc::new(f.clone())));
        }
        Item::Struct(s) => {
            let name = s.ident.to_string();
            let canon: Arc<str> = resolver.canon(m, &name).into();
            resolver.type_id(&canon);
            resolver.modules[m].structs.insert(name, canon.clone());
            resolver.structs.insert(
                canon,
                StructDef {
                    ast: Rc::new(s.clone()),
                    module: m,
                },
            );
        }
        Item::Enum(e) => {
            let name = e.ident.to_string();
            let canon: Arc<str> = resolver.canon(m, &name).into();
            resolver.modules[m].enums.insert(name, canon.clone());
            let type_id = resolver.type_id(&canon);
            let def = enum_def::EnumDef::new(
                enum_def::EnumKind::Other,
                canon.clone(),
                type_id,
                e.variants.iter().map(|v| {
                    (
                        Arc::from(v.ident.to_string()),
                        matches!(v.fields, syn::Fields::Unit),
                    )
                }),
            );
            resolver.enum_defs.insert(canon.clone(), def);
            resolver.enums.insert(canon, Rc::new(e.clone()));
        }
        Item::Impl(imp) => pending_impls.push((m, Rc::new(imp.clone()))),
        Item::Use(u) => {
            let syms = &mut resolver.modules[m];
            let mut prefix = Vec::new();
            collect_use_tree(&u.tree, &mut prefix, &mut syms.uses, &mut syms.globs);
        }
        Item::Const(c) => {
            resolver.modules[m].consts.insert(
                c.ident.to_string(),
                u32::try_from(pending_consts.len()).expect("table fits u32"),
            );
            pending_consts.push((m, Rc::new((*c.expr).clone()), Rc::new((*c.ty).clone())));
        }
        Item::Static(s) => {
            if matches!(s.mutability, syn::StaticMutability::Mut(_)) {
                bail!("unsupported feature: `static mut`");
            }
            resolver.modules[m].consts.insert(
                s.ident.to_string(),
                u32::try_from(pending_consts.len()).expect("table fits u32"),
            );
            pending_consts.push((m, Rc::new((*s.expr).clone()), Rc::new((*s.ty).clone())));
        }
        Item::Type(t) => {
            resolver.modules[m]
                .aliases
                .insert(t.ident.to_string(), Rc::new((*t.ty).clone()));
        }
        Item::Trait(_) => {}
        Item::Mod(_) => bail!("module declarations must be expanded by the loader"),
        other => bail!("unsupported item: {}", quote_kind(other)),
    }
    Ok(())
}

type PendingMethod = (String, String, usize, Rc<syn::ImplItemFn>);

/// The module, the initializer, and the declared type. The type is what gives an integer const its
/// width, without it a `const N: u16` would run as a plain untagged int and refuse to meet a real
/// `u16` in one operation.
type PendingConst = (usize, Rc<syn::Expr>, Rc<syn::Type>);

/// So an impl can pull in the default bodies it doesn't override.
fn collect_traits(modules: &[ModuleSrc]) -> HashMap<String, (usize, Rc<syn::ItemTrait>)> {
    let mut traits: HashMap<String, (usize, Rc<syn::ItemTrait>)> = HashMap::default();
    for (m, src) in modules.iter().enumerate() {
        for item in &src.items {
            if let Item::Trait(t) = item {
                traits.insert(t.ident.to_string(), (m, Rc::new(t.clone())));
            }
        }
    }
    traits
}

fn impl_name_tables(
    pending_methods: &[PendingMethod],
) -> (HashSet<(String, String)>, HashMap<String, u32>) {
    let impl_methods = pending_methods
        .iter()
        .map(|(ty, name, _, _)| (ty.clone(), name.clone()))
        .collect();
    let atoms = impls::method_atoms(pending_methods.iter().map(|(_, name, _, _)| name.as_str()));
    (impl_methods, atoms)
}

fn build_impl_table(
    resolver: &Resolver,
    methods: Vec<(String, String, Arc<Chunk>)>,
    method_atoms: &HashMap<String, u32>,
) -> Arc<impls::ImplTable> {
    let declared =
        |ty: &str| resolver.structs.contains_key(ty) || resolver.enum_defs.contains_key(ty);
    Arc::new(impls::ImplTable::build(
        methods,
        resolver.type_ids.clone(),
        method_atoms.clone(),
        &declared,
    ))
}

/// Whether any impl has `Drop::drop`, and the names of `&mut self` methods. A call to one compiles
/// its receiver as a place split from sharing. By name, because the runtime type is unknown at
/// compile time. An extra split is wasted work but never wrong.
fn collect_mut_methods(pending_methods: &[PendingMethod]) -> (bool, HashSet<String>) {
    let has_drop = pending_methods
        .iter()
        .any(|(_, name, _, _)| name == "Drop::drop");
    let mut_methods = pending_methods
        .iter()
        .filter(|(_, _, _, f)| {
            f.sig
                .receiver()
                .is_some_and(|r| matches!(r.kind, syn::ReceiverKind::Reference(_, _, Some(_))))
        })
        .map(|(_, name, _, _)| name.clone())
        .collect();
    (has_drop, mut_methods)
}

fn from_source_name(imp: &syn::ItemImpl) -> Option<String> {
    let (path, _) = imp.trait_.as_ref()?;
    let last = path.segments.last()?;
    if last.ident != "From" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    let ty = args.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    })?;
    from_type_key(ty)
}

/// Generic arguments are part of the key, `From<Option<usize>>` and `From<Option<u16>>` are
/// different impls.
fn from_type_key(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(p) => {
            let last = p.path.segments.last()?;
            let base = last.ident.to_string();
            let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
                return Some(base);
            };
            let inner: Vec<String> = args
                .args
                .iter()
                .filter_map(|arg| match arg {
                    syn::GenericArgument::Type(inner) => from_type_key(inner),
                    _ => None,
                })
                .collect();
            if inner.len() == args.args.len() {
                Some(format!("{base}<{}>", inner.join(",")))
            } else {
                Some(base)
            }
        }
        syn::Type::Reference(r) => from_type_key(&r.elem),
        syn::Type::Tuple(t) => {
            let inner: Vec<String> = t.elems.iter().filter_map(from_type_key).collect();
            (inner.len() == t.elems.len()).then(|| format!("({})", inner.join(",")))
        }
        _ => None,
    }
}

fn collect_impl_items(
    resolver: &mut Resolver,
    pending_impls: &[(usize, Rc<syn::ItemImpl>)],
    traits: &HashMap<String, (usize, Rc<syn::ItemTrait>)>,
    pending_consts: &mut Vec<PendingConst>,
) -> Result<Vec<PendingMethod>> {
    let mut pending_methods: Vec<PendingMethod> = Vec::new();
    for (m, imp) in pending_impls {
        let type_name = impl_target(resolver, *m, &imp.self_ty)
            .ok_or_else(|| anyhow!("unsupported impl target"))?;
        resolver.type_id(&type_name);
        let trait_name = imp
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .map(|seg| seg.ident.to_string());
        let mut written: Vec<String> = Vec::new();
        for it in &imp.items {
            match it {
                syn::ImplItem::Fn(f) => {
                    let method = f.sig.ident.to_string();
                    written.push(method.clone());
                    // `Display` and `Debug` both define `fmt`, so they are stored trait qualified.
                    // A plain `x.drop()` must never hit `Drop::drop`.
                    let key = match trait_name.as_deref() {
                        Some(t @ ("Display" | "Debug")) if method == "fmt" => {
                            format!("{t}::fmt")
                        }
                        Some("Drop") if method == "drop" => "Drop::drop".to_string(),
                        _ => method,
                    };
                    // also under `from<S>`, so several `From` impls don't clash
                    if key == "from"
                        && let Some(source) = from_source_name(imp)
                    {
                        pending_methods.push((
                            type_name.clone(),
                            format!("from<{source}>"),
                            *m,
                            Rc::new(f.clone()),
                        ));
                        // `None` can't name its payload, it still reaches a single impl through
                        // the bare outer name
                        if let Some(base) = source.split(['<', '(']).next()
                            && base != source
                        {
                            pending_methods.push((
                                type_name.clone(),
                                format!("from<{base}>"),
                                *m,
                                Rc::new(f.clone()),
                            ));
                        }
                    }
                    pending_methods.push((type_name.clone(), key, *m, Rc::new(f.clone())));
                }
                syn::ImplItem::Const(c) => {
                    let key = format!("{}::{}", resolver::bare(&type_name), c.ident);
                    resolver.modules[*m].consts.insert(
                        key,
                        u32::try_from(pending_consts.len()).expect("table fits u32"),
                    );
                    pending_consts.push((*m, Rc::new(c.expr.clone()), Rc::new(c.ty.clone())));
                }
                _ => {}
            }
        }
        if let Some((trait_module, def)) = trait_name.as_ref().and_then(|t| traits.get(t)) {
            for ti in &def.items {
                if let syn::TraitItem::Fn(tf) = ti
                    && let Some(body) = &tf.default
                    && !written.iter().any(|w| tf.sig.ident == w.as_str())
                {
                    let synthesized = syn::ImplItemFn {
                        attrs: tf.attrs.clone(),
                        vis: syn::Visibility::Inherited,
                        modifiers: syn::FnModifiers::default(),
                        sig: tf.sig.clone(),
                        block: body.clone(),
                    };
                    pending_methods.push((
                        type_name.clone(),
                        tf.sig.ident.to_string(),
                        *trait_module,
                        Rc::new(synthesized),
                    ));
                }
            }
        }
    }
    Ok(pending_methods)
}

/// The declared type of every `const` and `static`, by name, and `Type::NAME` for an impl const.
fn collect_const_types(modules: &[ModuleSrc], resolver: &Resolver) -> HashMap<String, syn::Type> {
    let mut out = HashMap::new();
    for (m, src) in modules.iter().enumerate() {
        for item in &src.items {
            match item {
                Item::Const(c) => {
                    out.insert(c.ident.to_string(), (*c.ty).clone());
                }
                Item::Static(s) => {
                    out.insert(s.ident.to_string(), (*s.ty).clone());
                }
                Item::Impl(imp) => {
                    let Some(ty) = impl_target(resolver, m, &imp.self_ty) else {
                        continue;
                    };
                    for it in &imp.items {
                        if let syn::ImplItem::Const(c) = it {
                            out.insert(
                                format!("{}::{}", resolver::bare(&ty), c.ident),
                                c.ty.clone(),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn impl_target(resolver: &Resolver, m: usize, ty: &syn::Type) -> Option<String> {
    let syn::Type::Path(p) = ty else { return None };
    let segs: Vec<String> = p
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    match resolver.resolve(m, &segs) {
        Ok(Res::Struct(c) | Res::Enum(c)) => Some(c.to_string()),
        // Builtins are keyed by the written type with generics, `Vec<u8>` and `Vec<String>` are
        // different keys. See `ImplTable::of_builtin`.
        _ => Some(foreign_impl_key(&p.path)),
    }
}

/// `Vec<String>` for `std::vec::Vec<String>` too, without the token printer whitespace.
fn foreign_impl_key(path: &syn::Path) -> String {
    let last = path.segments.last().expect("a type path has a segment");
    last.to_token_stream()
        .to_string()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn collect_use_tree(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    out: &mut HashMap<String, Vec<String>>,
    globs: &mut Vec<Vec<String>>,
) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            collect_use_tree(&p.tree, prefix, out, globs);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name == "self" {
                if let Some(last) = prefix.last() {
                    out.insert(last.clone(), prefix.clone());
                }
                return;
            }
            let mut full = prefix.clone();
            full.push(name.clone());
            out.insert(name, full);
        }
        syn::UseTree::Rename(r) => {
            let mut full = prefix.clone();
            full.push(r.ident.to_string());
            out.insert(r.rename.to_string(), full);
        }
        syn::UseTree::Group(g) => {
            for item in &g.items {
                collect_use_tree(item, prefix, out, globs);
            }
        }
        syn::UseTree::Glob(_) => globs.push(prefix.clone()),
    }
}

fn quote_kind(item: &Item) -> &'static str {
    match item {
        Item::Fn(_) => "fn",
        Item::Struct(_) => "struct",
        Item::Enum(_) => "enum",
        Item::Impl(_) => "impl",
        Item::Trait(_) => "trait",
        Item::Macro(_) => "macro",
        Item::Union(_) => "union",
        _ => "item",
    }
}
