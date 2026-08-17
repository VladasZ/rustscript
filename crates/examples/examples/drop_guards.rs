#!/usr/bin/env rust

// User `Drop` impls run when a binding's scope ends, in reverse declaration
// order, on explicit `drop`, and per loop iteration.

struct Guard {
    name: String,
}

impl Drop for Guard {
    fn drop(&mut self) {
        println!("dropping {}", self.name);
    }
}

fn scoped() {
    let _inner = Guard {
        name: "inner".to_string(),
    };
    println!("in scoped");
}

fn main() {
    let _a = Guard {
        name: "a".to_string(),
    };
    let _b = Guard {
        name: "b".to_string(),
    };
    {
        let _block = Guard {
            name: "block".to_string(),
        };
        println!("in block");
    }
    scoped();
    let early = Guard {
        name: "early".to_string(),
    };
    drop(early);
    println!("after drop");
    for i in 0..2 {
        let _per_loop = Guard {
            name: format!("loop{i}"),
        };
        if i == 1 {
            break;
        }
    }
    println!("end of main");
}
