use std::time::Instant;

struct Body {
    x: f64,
    y: f64,
    z: f64,
    vx: f64,
    vy: f64,
    vz: f64,
    mass: f64,
}

fn bodies() -> Vec<Body> {
    let pi = 3.141592653589793;
    let solar_mass = 4.0 * pi * pi;
    let days = 365.24;
    vec![
        Body { x: 0.0, y: 0.0, z: 0.0, vx: 0.0, vy: 0.0, vz: 0.0, mass: solar_mass },
        Body {
            x: 4.84143144246472090,
            y: -1.16032004402742839,
            z: -0.103622044471123109,
            vx: 0.00166007664274403694 * days,
            vy: 0.00769901118419740425 * days,
            vz: -0.0000690460016972063023 * days,
            mass: 0.000954791938424326609 * solar_mass,
        },
        Body {
            x: 8.34336671824457987,
            y: 4.12479856412430479,
            z: -0.403523417114321381,
            vx: -0.00276742510726862411 * days,
            vy: 0.00499852801234917238 * days,
            vz: 0.0000230417297573763929 * days,
            mass: 0.000285885980666130812 * solar_mass,
        },
        Body {
            x: 12.8943695621391310,
            y: -15.1111514016986312,
            z: -0.223307578892655734,
            vx: 0.00296460137564761618 * days,
            vy: 0.00237847173959480950 * days,
            vz: -0.0000296589568540237556 * days,
            mass: 0.0000436624404335156298 * solar_mass,
        },
        Body {
            x: 15.3796971148509165,
            y: -25.9193146099879641,
            z: 0.179258772950371181,
            vx: 0.00268067772490389322 * days,
            vy: 0.00162824170038242295 * days,
            vz: -0.0000951592254519715870 * days,
            mass: 0.0000515138902046611451 * solar_mass,
        },
    ]
}

fn energy(bodies: &Vec<Body>) -> f64 {
    let n = bodies.len();
    let mut e = 0.0;
    let mut i = 0;
    while i < n {
        let b = &bodies[i];
        e += 0.5 * b.mass * (b.vx * b.vx + b.vy * b.vy + b.vz * b.vz);
        let mut j = i + 1;
        while j < n {
            let dx = b.x - bodies[j].x;
            let dy = b.y - bodies[j].y;
            let dz = b.z - bodies[j].z;
            let d = (dx * dx + dy * dy + dz * dz).sqrt();
            e -= b.mass * bodies[j].mass / d;
            j += 1;
        }
        i += 1;
    }
    e
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let steps: i64 = if args.len() > 1 { args[1].parse().unwrap() } else { 8_000 };
    let t = Instant::now();
    let mut bodies = bodies();
    let n = bodies.len();

    // Offset the sun so total momentum is zero.
    let mut px = 0.0;
    let mut py = 0.0;
    let mut pz = 0.0;
    for b in &bodies {
        px += b.vx * b.mass;
        py += b.vy * b.mass;
        pz += b.vz * b.mass;
    }
    let solar_mass = bodies[0].mass;
    bodies[0].vx = -px / solar_mass;
    bodies[0].vy = -py / solar_mass;
    bodies[0].vz = -pz / solar_mass;

    let e0 = energy(&bodies);
    let dt = 0.01;
    for _ in 0..steps {
        let mut i = 0;
        while i < n {
            let mut j = i + 1;
            while j < n {
                let dx = bodies[i].x - bodies[j].x;
                let dy = bodies[i].y - bodies[j].y;
                let dz = bodies[i].z - bodies[j].z;
                let d2 = dx * dx + dy * dy + dz * dz;
                let mag = dt / (d2 * d2.sqrt());
                let mi = bodies[i].mass * mag;
                let mj = bodies[j].mass * mag;
                bodies[i].vx -= dx * mj;
                bodies[i].vy -= dy * mj;
                bodies[i].vz -= dz * mj;
                bodies[j].vx += dx * mi;
                bodies[j].vy += dy * mi;
                bodies[j].vz += dz * mi;
                j += 1;
            }
            i += 1;
        }
        let mut k = 0;
        while k < n {
            let vx = bodies[k].vx;
            let vy = bodies[k].vy;
            let vz = bodies[k].vz;
            bodies[k].x += dt * vx;
            bodies[k].y += dt * vy;
            bodies[k].z += dt * vz;
            k += 1;
        }
    }
    let e1 = energy(&bodies);
    let ns = t.elapsed().as_nanos();
    println!("start {} end {}", (e0 * 1e9).round() as i64, (e1 * 1e9).round() as i64);
    eprintln!("COMPUTE_NS {ns}");
}
