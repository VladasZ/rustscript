//! Struct elements in vecs through the while plan's element handles, sharing
//! edges included.

struct Body {
    x: f64,
    v: f64,
    hits: i64,
}

#[derive(Clone)]
struct Point {
    x: i64,
    y: i64,
}

struct Tagged {
    id: i64,
    name: String,
}

fn main() {
    // An nbody style pair interaction.
    let mut bodies = vec![
        Body {
            x: 1.5,
            v: 0.0,
            hits: 0,
        },
        Body {
            x: -2.0,
            v: 0.5,
            hits: 0,
        },
        Body {
            x: 4.0,
            v: -1.0,
            hits: 0,
        },
    ];
    let total = bodies.len();
    let mut step = 0;
    while step < 200 {
        let mut i = 0;
        while i < total {
            let mut j = i + 1;
            while j < total {
                let dx = bodies[i].x - bodies[j].x;
                let pull = dx / (dx * dx).sqrt() * 0.001;
                bodies[i].v -= pull;
                bodies[j].v += pull;
                j += 1;
            }
            i += 1;
        }
        let mut walk = 0;
        while walk < total {
            let speed = bodies[walk].v;
            bodies[walk].x += speed * 0.01;
            bodies[walk].hits += 1;
            walk += 1;
        }
        step += 1;
    }
    let mut checksum = 0.0;
    let mut total_hits: i64 = 0;
    for body in bodies {
        checksum += body.x * 1000.0 + body.v;
        total_hits += body.hits;
    }
    println!("checksum {} hits {total_hits}", checksum.round());

    // A clone taken before the loop keeps its values.
    let mut points = [Point { x: 1, y: 10 }, Point { x: 2, y: 20 }];
    let kept = points[0].clone();
    let mut rounds = 0;
    while rounds < 50 {
        points[0].x += 1;
        points[0].y += points[1].y;
        rounds += 1;
    }
    println!(
        "kept {} {} grown {} {}",
        kept.x, kept.y, points[0].x, points[0].y
    );

    // 2 slots holding one shared element split on their first writes.
    let seed = Point { x: 100, y: 0 };
    let mut pair = [seed.clone(), seed];
    let mut turns = 0;
    while turns < 30 {
        pair[0].x += 1;
        pair[1].x += 2;
        turns += 1;
    }
    println!("pair {} {}", pair[0].x, pair[1].x);

    // A string field read fails the iteration over. The id write is undone and
    // re-applied, so the increment lands once.
    let mut tags = [
        Tagged {
            id: 1,
            name: String::from("one"),
        },
        Tagged {
            id: 2,
            name: String::from("two"),
        },
    ];
    let high = String::from("zzz");
    let tag_count = tags.len();
    let mut sum: i64 = 0;
    let mut named = 0;
    let mut ti = 0;
    while ti < tag_count {
        tags[ti].id += 10;
        sum += tags[ti].id;
        if tags[ti].name < high {
            named += 1;
        }
        ti += 1;
    }
    println!("sum {sum} named {named}");
}
