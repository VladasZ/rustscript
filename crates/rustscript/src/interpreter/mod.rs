mod assoc;
mod bridge;
mod bytecode;
mod cell;
mod compile;
mod console;
pub mod coverage;
mod crates_bridge;
mod format;
mod higher_order;
mod http;
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
mod scalar;
mod scalar_chain;
mod scalar_fn;
mod scalar_fold;
mod scalar_for;
mod scalar_loop;
mod scalar_reads;
mod scalar_val;
mod scalar_while;
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
use syn::Item;

use crate::loader::ModuleSrc;
use bytecode::Chunk;
use compile::{Compiler, Ctx};
use resolver::{ModuleSyms, Res, Resolver, StructDef};
pub use vm_support::{ErrReturn, ScriptPanic};

/// Set by the real Ctrl-C handler, which must stay `Send`, and drained by the
/// interpreter between loop iterations so it can run the script's own handler.
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

/// The script's Ctrl-C handler when a Ctrl-C is pending, None otherwise.
/// Draining the flag here means each Ctrl-C runs the handler once.
pub(crate) fn pending_ctrlc_handler() -> Option<value::Value> {
    // Cheap relaxed load first, this runs on every loop iteration.
    if !CTRLC_HIT.load(Ordering::Relaxed) {
        return None;
    }
    if !CTRLC_HIT.swap(false, Ordering::SeqCst) {
        return None;
    }
    CTRLC_HANDLER.lock().clone()
}

/// The arguments a script sees through `std::env::args()`. Index 0 is the
/// script path, matching a real compiled binary.
static SCRIPT_ARGS: OnceLock<Vec<String>> = OnceLock::new();

pub fn set_script_args(args: Vec<String>) {
    SCRIPT_ARGS
        .set(args)
        .expect("script args are set exactly once");
}

pub(crate) fn script_args() -> Vec<String> {
    SCRIPT_ARGS.get().cloned().unwrap_or_default()
}

/// Run a program on a multi thread tokio runtime. `async_mode` marks the
/// script as `#[tokio::main]`, which is what allows `.await`, `tokio::spawn`,
/// and `join!` to compile.
pub fn run(modules: &[ModuleSrc], async_mode: bool) -> Result<()> {
    let interp = Interp::load(modules, async_mode)?;
    // The coverage walk runs before execution, so an unchecked script cannot
    // die on a cold branch after doing half its side effects. It is one
    // linear pass over the compiled bytecode, measured at well under a
    // millisecond even on the thousand-line bench cases.
    interp.coverage_gate()?;
    interp.run()
}

/// A module level const or static: compiled once, evaluated on first read.
enum GlobalSlot {
    Todo(Arc<Chunk>),
}

/// The whole program, compiled to bytecode and ready to run.
pub struct Interp {
    /// Every function of every module, indexed by id. Direct calls use the id.
    functions: Vec<Arc<Chunk>>,
    /// Canonical name to function id, for calls resolved at runtime.
    fn_index: HashMap<String, u32>,
    /// Inherent and trait methods, keyed by (canonical type name, method name).
    methods: HashMap<(String, String), Arc<Chunk>>,
    /// Module tree and item tables, shared by compile and runtime lookups.
    resolver: Resolver,
    /// Consts and statics, evaluated lazily so declaration order is free.
    globals: RefCell<Vec<GlobalSlot>>,
    /// Root module imports, used by the bridge dispatch to expand aliases.
    uses: HashMap<String, Vec<String>>,
    main_index: Option<u32>,
    /// Whether an `Err` out of `main` prints its `Display` text rather than
    /// its `Debug` form, decided by `main`'s written error type.
    main_err_display: bool,
}

/// Whether `main`'s declared error type prints as `Display` when it falls out
/// of `main`. Real Rust prints the `Debug` form of the error value, and the
/// one common exception is `anyhow::Error`, whose own `Debug` prints the bare
/// message. `anyhow::Result<()>` carries no written error type, so a `Result`
/// with fewer than two type arguments is the anyhow shape too, unless the
/// imports say the name means something else.
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
        // No written error type. A plain `Result<()>` resolves through the
        // imports, `use anyhow::Result` being the common way to write it.
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

/// Stated return scalars of the script's own functions, one more place a
/// `Default` payload is written down. A name defined in more than one module
/// with differing returns answers for neither.
fn collect_fn_returns(
    pending_fns: &[(usize, Rc<syn::ItemFn>)],
) -> HashMap<String, bytecode::ScalarTy> {
    let mut seen_returns: HashMap<String, Option<bytecode::ScalarTy>> = HashMap::default();
    for (_, f) in pending_fns {
        let lowered = match &f.sig.output {
            syn::ReturnType::Type(_, ty) => bytecode::ScalarTy::lower(ty),
            syn::ReturnType::Default => None,
        };
        seen_returns
            .entry(f.sig.ident.to_string())
            .and_modify(|known| {
                if *known != lowered {
                    *known = None;
                }
            })
            .or_insert(lowered);
    }
    seen_returns
        .into_iter()
        .filter_map(|(name, scalar)| scalar.map(|s| (name, s)))
        .collect()
}

/// Every function under its full `module::name` key, bare names at the root.
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
        let mut pending_consts: Vec<(usize, Rc<syn::Expr>)> = Vec::new();

        // Trait definitions by bare name, so an impl can pull in the default
        // method bodies its block does not override.
        let mut traits: HashMap<String, (usize, Rc<syn::ItemTrait>)> = HashMap::default();
        for (m, src) in modules.iter().enumerate() {
            for item in &src.items {
                if let Item::Trait(t) = item {
                    traits.insert(t.ident.to_string(), (m, Rc::new(t.clone())));
                }
            }
        }

        for (m, src) in modules.iter().enumerate() {
            for item in &src.items {
                register_item(
                    &mut resolver,
                    m,
                    item,
                    &mut pending_fns,
                    &mut pending_impls,
                    &mut pending_consts,
                )?;
            }
        }
        resolver.reject_module_globs()?;

        let pending_methods =
            collect_impl_items(&mut resolver, &pending_impls, &traits, &mut pending_consts)?;

        let fn_returns = collect_fn_returns(&pending_fns);

        let (has_drop, mut_methods) = collect_mut_methods(&pending_methods);

        let mut functions = Vec::with_capacity(pending_fns.len());
        for (m, f) in &pending_fns {
            let ctx = Ctx {
                resolver: &resolver,
                module: *m,
                file: modules[*m].file.clone(),
                async_mode,
                impl_type: None,
                fn_returns: &fn_returns,
                mut_methods: &mut_methods,
                has_drop,
            };
            let mut c = Compiler::new(&ctx);
            functions.push(Arc::new(c.compile_fn(&f.sig, &f.block)?));
        }
        let mut methods = HashMap::default();
        for (ty, name, m, f) in &pending_methods {
            let ctx = Ctx {
                resolver: &resolver,
                module: *m,
                file: modules[*m].file.clone(),
                async_mode,
                impl_type: Some(ty),
                fn_returns: &fn_returns,
                mut_methods: &mut_methods,
                has_drop,
            };
            let mut c = Compiler::new(&ctx);
            methods.insert(
                (ty.clone(), name.clone()),
                Arc::new(c.compile_fn(&f.sig, &f.block)?),
            );
        }
        let mut globals = Vec::with_capacity(pending_consts.len());
        for (m, expr) in &pending_consts {
            let ctx = Ctx {
                resolver: &resolver,
                module: *m,
                file: modules[*m].file.clone(),
                async_mode,
                impl_type: None,
                fn_returns: &fn_returns,
                mut_methods: &mut_methods,
                has_drop,
            };
            let mut c = Compiler::new(&ctx);
            globals.push(GlobalSlot::Todo(Arc::new(c.compile_const(expr)?)));
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
            methods,
            resolver,
            globals: RefCell::new(globals),
            uses,
            main_index,
            main_err_display,
        })
    }

    /// Run `fn main`. Its returned `Result::Err` is reported like anyhow does.
    /// Report methods the interpreter does not implement, without running
    /// anything. Used by `rust check`.
    pub fn coverage(&self) -> Vec<coverage::Finding> {
        let user = self.methods.keys().cloned();
        coverage::report(&self.functions, user)
    }

    /// The coverage walk as a gate: an error listing every method the
    /// interpreter does not implement, or `Ok` when it implements them all.
    /// `rust check` and every interpreted run share this exact report.
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

    /// Run `main` as a blocking task on a multi thread tokio runtime, so
    /// awaited futures can be driven with `block_on` from a worker thread.
    fn run(&self) -> Result<()> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow!("cannot start tokio runtime: {e}"))?;
        let functions = self.functions.clone();
        let methods = self.methods.clone();
        let globals: Vec<parking_lot::Mutex<vm::GlobalSlot>> = self
            .globals
            .borrow()
            .iter()
            .map(|slot| {
                let GlobalSlot::Todo(c) = slot;
                parking_lot::Mutex::new(vm::GlobalSlot::Todo(c.clone()))
            })
            .collect();
        // The runtime tables for dynamic dispatch, precomputed here so nothing
        // at runtime touches the syn AST, which is not `Send`.
        let enums: Vec<vm::EnumDef> = self
            .resolver
            .enums
            .iter()
            .map(|(name, def)| vm::EnumDef {
                name: Arc::from(&**name),
                variants: def
                    .variants
                    .iter()
                    .map(|v| {
                        (
                            Arc::from(v.ident.to_string().as_str()),
                            matches!(v.fields, syn::Fields::Unit),
                        )
                    })
                    .collect(),
            })
            .collect();
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
            methods,
            globals,
            structs: self.build_structs(),
            uses: self.uses.clone(),
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
        // `main` runs on a plain thread rather than a blocking task, so it
        // consumes no tokio task id and the script's first `tokio::spawn`
        // gets the same id a compiled binary's first spawn gets. Awaits
        // still work: the runtime keeps driving itself and the thread
        // enters it through the stored handle.
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
        if let value::Value::Enum {
            enum_name,
            variant,
            data,
        } = &ret
            && &**enum_name == "Result"
            && &**variant == "Err"
        {
            // A compiled binary prints the `Debug` form of the error here.
            // An `anyhow::Error` is the exception, its own `Debug` prints
            // the bare message, which `display` mirrors.
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

/// Build the empty module table with parent and child links.
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
    }
}

fn register_item(
    resolver: &mut Resolver,
    m: usize,
    item: &Item,
    pending_fns: &mut Vec<(usize, Rc<syn::ItemFn>)>,
    pending_impls: &mut Vec<(usize, Rc<syn::ItemImpl>)>,
    pending_consts: &mut Vec<(usize, Rc<syn::Expr>)>,
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
            pending_consts.push((m, Rc::new((*c.expr).clone())));
        }
        Item::Static(s) => {
            if matches!(s.mutability, syn::StaticMutability::Mut(_)) {
                bail!("unsupported feature: `static mut`");
            }
            resolver.modules[m].consts.insert(
                s.ident.to_string(),
                u32::try_from(pending_consts.len()).expect("table fits u32"),
            );
            pending_consts.push((m, Rc::new((*s.expr).clone())));
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

/// One method to compile: its target type, method key, defining module,
/// and body.
type PendingMethod = (String, String, usize, Rc<syn::ImplItemFn>);

/// Register every impl block's methods and consts, resolving impl targets
/// after all modules registered their types. A trait impl also brings in the
/// trait's default bodies for methods it does not override, compiled against
/// the trait's module.
/// Whether any impl defines `Drop::drop`, and the method names any impl
/// declares with a `&mut self` receiver. A call to one of the latter compiles
/// its receiver as a place, split from value sharing first, so the mutation
/// stays private to the receiver. The set is by name because the receiver's
/// runtime type is not known at compile time; splitting a receiver that
/// resolves to a `&self` method of the same name is wasted work, never wrong.
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

fn collect_impl_items(
    resolver: &mut Resolver,
    pending_impls: &[(usize, Rc<syn::ItemImpl>)],
    traits: &HashMap<String, (usize, Rc<syn::ItemTrait>)>,
    pending_consts: &mut Vec<(usize, Rc<syn::Expr>)>,
) -> Result<Vec<PendingMethod>> {
    let mut pending_methods: Vec<PendingMethod> = Vec::new();
    for (m, imp) in pending_impls {
        let type_name = impl_target(resolver, *m, &imp.self_ty)
            .ok_or_else(|| anyhow!("unsupported impl target"))?;
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
                    // Both `Display` and `Debug` define `fmt`, so their impls
                    // are stored trait-qualified and looked up by the
                    // formatter, never by a plain method call. `Drop::drop`
                    // runs only at end of life, a plain `x.drop()` call must
                    // never reach it.
                    let key = match trait_name.as_deref() {
                        Some(t @ ("Display" | "Debug")) if method == "fmt" => {
                            format!("{t}::fmt")
                        }
                        Some("Drop") if method == "drop" => "Drop::drop".to_string(),
                        _ => method,
                    };
                    pending_methods.push((type_name.clone(), key, *m, Rc::new(f.clone())));
                }
                syn::ImplItem::Const(c) => {
                    let key = format!("{}::{}", resolver::bare(&type_name), c.ident);
                    resolver.modules[*m].consts.insert(
                        key,
                        u32::try_from(pending_consts.len()).expect("table fits u32"),
                    );
                    pending_consts.push((*m, Rc::new(c.expr.clone())));
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

/// Canonical name of the type an `impl` block targets.
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
        // An impl on something else, a bridge type name for example, keeps
        // the old bare name behavior.
        _ => segs.last().cloned(),
    }
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
                // `use a::b::{self}` imports the module under its own name.
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
