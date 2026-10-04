//! A `use` inside a fn body. The body compiles against a copy of its module that also holds
//! those imports.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::{Result, bail};
use syn::visit::Visit;

use super::register::{PendingMethod, collect_use_tree};
use super::resolver::{ModuleSyms, Resolver};

/// Points every fn and method whose body imports something at its own copy of the module.
pub(super) fn scope_local_uses(
    resolver: &mut Resolver,
    fns: &mut [(usize, Rc<syn::ItemFn>)],
    methods: &mut [PendingMethod],
) -> Result<()> {
    for (module, item) in fns {
        if let Some(scope) = local_scope(resolver, *module, &item.block)? {
            *module = scope;
        }
    }
    for (_, _, module, item) in methods {
        if let Some(scope) = local_scope(resolver, *module, &item.block)? {
            *module = scope;
        }
    }
    Ok(())
}

/// The copy covers the whole body. `rustc` scopes an import to its block, so one name that 2
/// blocks of a body import from 2 places is refused instead of resolved wrong.
fn local_scope(
    resolver: &mut Resolver,
    module: usize,
    block: &syn::Block,
) -> Result<Option<usize>> {
    let mut found = LocalUses::default();
    found.visit_block(block);
    if found.trees.is_empty() {
        return Ok(None);
    }
    let mut scope = resolver.modules[module].clone();
    scope.scope_of = Some(resolver.modules[module].scope_of.unwrap_or(module));
    let mut local: HashMap<String, Vec<String>> = HashMap::new();
    for tree in &found.trees {
        let mut one = HashMap::new();
        collect_use_tree(tree, &mut Vec::new(), &mut one, &mut scope.globs);
        for (name, target) in one {
            if local.get(&name).is_some_and(|seen| *seen != target) {
                bail!(
                    "unsupported feature: one function body imports 2 different items as `{name}`"
                );
            }
            local.insert(name, target);
        }
    }
    for (name, target) in local {
        // an import in a body hides the item of the module with the same name
        scope.forget(&name);
        scope.uses.insert(name, target);
    }
    resolver.modules.push(scope);
    Ok(Some(resolver.modules.len() - 1))
}

#[derive(Default)]
struct LocalUses {
    trees: Vec<syn::UseTree>,
}

impl<'ast> Visit<'ast> for LocalUses {
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.trees.push(item.tree.clone());
    }
}

impl ModuleSyms {
    fn forget(&mut self, name: &str) {
        self.fns.remove(name);
        self.consts.remove(name);
        self.structs.remove(name);
        self.enums.remove(name);
        self.aliases.remove(name);
        self.children.remove(name);
    }
}
