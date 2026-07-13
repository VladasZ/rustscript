use std::f64::consts::PI;
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
    let solar_mass = 4.0 * PI * PI;
    let days = 365.24;
    vec![
        Body {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            vx: 0.0,
            vy: 0.0,
            vz: 0.0,
            mass: solar_mass,
        },
        Body {
            x: 4.841_431_442_464_721,
            y: -1.160_320_044_027_428_4,
            z: -0.103_622_044_471_123_11,
            vx: 0.001_660_076_642_744_037 * days,
            vy: 0.007_699_011_184_197_404 * days,
            vz: -0.000_069_046_001_697_206_3 * days,
            mass: 0.000_954_791_938_424_326_6 * solar_mass,
        },
        Body {
            x: 8.343_366_718_244_58,
            y: 4.124_798_564_124_305,
            z: -0.403_523_417_114_321_4,
            vx: -0.002_767_425_107_268_624 * days,
            vy: 0.004_998_528_012_349_172 * days,
            vz: 0.000_023_041_729_757_376_393 * days,
            mass: 0.000_285_885_980_666_130_8 * solar_mass,
        },
        Body {
            x: 12.894_369_562_139_131,
            y: -15.111_151_401_698_631,
            z: -0.223_307_578_892_655_73,
            vx: 0.002_964_601_375_647_616 * days,
            vy: 0.002_378_471_739_594_809_5 * days,
            vz: -0.000_029_658_956_854_023_756 * days,
            mass: 0.000_043_662_440_433_515_63 * solar_mass,
        },
        Body {
            x: 15.379_697_114_850_917,
            y: -25.919_314_609_987_964,
            z: 0.179_258_772_950_371_18,
            vx: 0.002_680_677_724_903_893_2 * days,
            vy: 0.001_628_241_700_382_423 * days,
            vz: -0.000_095_159_225_451_971_59 * days,
            mass: 0.000_051_513_890_204_661_145 * solar_mass,
        },
    ]
}

fn energy(bodies: &[Body]) -> f64 {
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
    let steps: i64 = if args.len() > 1 {
        args[1].parse().unwrap()
    } else {
        8_000
    };
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
    println!(
        "start {} end {}",
        (e0 * 1e9).round() as i64,
        (e1 * 1e9).round() as i64
    );
    eprintln!("COMPUTE_NS {ns}");
}
