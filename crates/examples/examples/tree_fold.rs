#!/usr/bin/env rust

// Self-recursive functions that build and fold a recursive enum run as one
// function plan, see interpreter/scalar_fn.rs. The traps are equivalence of
// the plan's enum ops with the generic path: construction, match arm
// selection, payload binding, values crossing the plan boundary both ways,
// and a consumed payload never being read again. A wildcard pattern is not
// in the plan subset, so left_depth pins the generic path on the same data.

enum Tree {
    Leaf,
    Node(Box<Tree>, Box<Tree>),
}

fn make(depth: i64) -> Tree {
    if depth == 0 {
        Tree::Leaf
    } else {
        Tree::Node(Box::new(make(depth - 1)), Box::new(make(depth - 1)))
    }
}

fn count(t: &Tree) -> i64 {
    match t {
        Tree::Leaf => 1,
        Tree::Node(l, r) => 1 + count(l) + count(r),
    }
}

fn lopsided(depth: i64) -> Tree {
    if depth == 0 {
        Tree::Leaf
    } else {
        Tree::Node(Box::new(lopsided(depth - 1)), Box::new(Tree::Leaf))
    }
}

// Takes the tree by value and returns a new one, so plan-built values cross
// the boundary in both directions and every bound payload is consumed.
fn mirror(t: Tree) -> Tree {
    match t {
        Tree::Leaf => Tree::Leaf,
        Tree::Node(l, r) => Tree::Node(Box::new(mirror(*r)), Box::new(mirror(*l))),
    }
}

// A wildcard element is outside the plan subset, so this runs generically.
fn left_depth(t: &Tree) -> i64 {
    match t {
        Tree::Leaf => 0,
        Tree::Node(l, _) => 1 + left_depth(l),
    }
}

// Payloads mixing scalars and subtrees, across four variants.
enum Expr {
    Num(i64),
    Add(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
}

fn build_expr(n: i64) -> Expr {
    if n == 0 {
        Expr::Num(1)
    } else if n % 3 == 0 {
        Expr::Neg(Box::new(build_expr(n - 1)))
    } else if n % 3 == 1 {
        Expr::Add(Box::new(build_expr(n - 1)), Box::new(Expr::Num(n)))
    } else {
        Expr::Mul(Box::new(build_expr(n - 1)), Box::new(Expr::Num(n)))
    }
}

fn eval(e: &Expr) -> i64 {
    match e {
        Expr::Num(n) => *n,
        Expr::Add(a, b) => eval(a) + eval(b),
        Expr::Mul(a, b) => eval(a) * eval(b),
        Expr::Neg(a) => -eval(a),
    }
}

enum IntList {
    Nil,
    Cons(i64, Box<IntList>),
}

fn build_list(n: i64) -> IntList {
    if n == 0 {
        IntList::Nil
    } else {
        IntList::Cons(n, Box::new(build_list(n - 1)))
    }
}

fn sum_list(l: &IntList) -> i64 {
    match l {
        IntList::Nil => 0,
        IntList::Cons(v, rest) => v + sum_list(rest),
    }
}

fn main() {
    let t = make(4);
    println!("count {}", count(&t));
    // The same tree again, so the returned value outlives its first use.
    println!("count again {}", count(&t));

    let lop = lopsided(5);
    println!("left {}", left_depth(&lop));
    let m = mirror(lop);
    println!("left mirrored {}", left_depth(&m));
    println!("count mirrored {}", count(&m));

    let e = build_expr(7);
    println!("eval {}", eval(&e));

    let l = build_list(10);
    println!("sum {}", sum_list(&l));

    for d in 0..6 {
        println!("nodes {}", count(&make(d)));
    }
}
