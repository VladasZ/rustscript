#!/usr/bin/env rust

//! Conformance program for multifile scripts. Compiled by cargo and run by
//! the rustscript interpreter, the test asserts both print identical output.
//! It exercises a nested module tree, every import style, re-export chains,
//! and cross module structs, enums, consts, statics, and type aliases.

mod data;
mod geometry;
mod prelude;
mod text {
    pub fn shout(s: &str) -> String {
        s.to_uppercase()
    }
    pub mod inner {
        pub fn decorate(s: &str) -> String {
            format!("<<{s}>>")
        }
        pub fn via_super(s: &str) -> String {
            super::shout(s)
        }
    }
}

use crate::data::DEFAULT_TAG;
use crate::data::models::{Item, Kind as ItemKind, Rating};
use crate::data::store::Store;
use crate::geometry::{
    ops::transform::{scale, translate},
    shapes::{self, Circle, Origin, Pair},
};
use prelude::{Area, ItemAlias, ORIGIN_X, Rect, project_name};
use text::inner::{decorate, via_super};

const LIMIT: i64 = ORIGIN_X + 40;
static GREETING: &str = "conformance";

type Grid = Vec<Vec<i64>>;
type R = Rect;

fn main() {
    println!("{} says {}", project_name(), GREETING);
    println!("limit {LIMIT}");
    println!("shapes from the {} set", shapes::SHAPE_SET);

    let c = Circle::new(3);
    let r = Rect { w: 4, h: 5 };
    println!("circle area {}", c.area());
    println!("rect area {}", r.area());
    println!("unit area {}", shapes::UNIT.area());
    let far = crate::geometry::shapes::Circle::new(10);
    println!("far area {}", far.area());

    let moved = translate(&r, 2, 3);
    let big = scale(&moved, 2);
    println!("big {big:?}");

    let r0 = R { w: 6, h: 7 };
    println!("aliased rect area {}", R::area(&r0));

    let pair = Pair(7, 8);
    println!("pair {:?} sums to {}", pair, pair.sum());
    println!("origin {Origin:?}");

    let mut store = Store::new();
    store.add(Item::new(1, "hammer", ItemKind::Tool));
    store.add(Item::new(2, "apple", ItemKind::Food));
    let roped: ItemAlias = Item::new(3, "rope", ItemKind::Tool);
    store.add(roped);
    println!("count {}", store.len());
    println!("tools {}", store.count_kind(ItemKind::Tool));
    for it in store.items() {
        println!("- {}", it.describe());
    }
    println!("tag {DEFAULT_TAG}");

    let rating = Rating::Stars(4);
    match rating {
        Rating::Stars(n) => println!("stars {n}"),
        Rating::Unrated => println!("unrated"),
    }
    println!("fallback {:?}", Rating::Unrated);

    let grid: Grid = vec![vec![1, 2], vec![3, 4]];
    let mut total = 0;
    for row in &grid {
        for v in row {
            total += v;
        }
    }
    println!("grid total {total}");

    let area: Area = r.area();
    println!("area alias {area}");

    println!("{}", decorate(&via_super("done")));
}
