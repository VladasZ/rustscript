type Body = { x: number; y: number; z: number; vx: number; vy: number; vz: number; mass: number };

function makeBodies(): Body[] {
  const pi = 3.141592653589793;
  const solarMass = 4.0 * pi * pi;
  const days = 365.24;
  return [
    { x: 0.0, y: 0.0, z: 0.0, vx: 0.0, vy: 0.0, vz: 0.0, mass: solarMass },
    {
      x: 4.84143144246472090,
      y: -1.16032004402742839,
      z: -0.103622044471123109,
      vx: 0.00166007664274403694 * days,
      vy: 0.00769901118419740425 * days,
      vz: -0.0000690460016972063023 * days,
      mass: 0.000954791938424326609 * solarMass,
    },
    {
      x: 8.34336671824457987,
      y: 4.12479856412430479,
      z: -0.403523417114321381,
      vx: -0.00276742510726862411 * days,
      vy: 0.00499852801234917238 * days,
      vz: 0.0000230417297573763929 * days,
      mass: 0.000285885980666130812 * solarMass,
    },
    {
      x: 12.8943695621391310,
      y: -15.1111514016986312,
      z: -0.223307578892655734,
      vx: 0.00296460137564761618 * days,
      vy: 0.00237847173959480950 * days,
      vz: -0.0000296589568540237556 * days,
      mass: 0.0000436624404335156298 * solarMass,
    },
    {
      x: 15.3796971148509165,
      y: -25.9193146099879641,
      z: 0.179258772950371181,
      vx: 0.00268067772490389322 * days,
      vy: 0.00162824170038242295 * days,
      vz: -0.0000951592254519715870 * days,
      mass: 0.0000515138902046611451 * solarMass,
    },
  ];
}

function energy(bodies: Body[]): number {
  const n = bodies.length;
  let e = 0.0;
  for (let i = 0; i < n; i++) {
    const b = bodies[i];
    e += 0.5 * b.mass * (b.vx * b.vx + b.vy * b.vy + b.vz * b.vz);
    for (let j = i + 1; j < n; j++) {
      const dx = b.x - bodies[j].x;
      const dy = b.y - bodies[j].y;
      const dz = b.z - bodies[j].z;
      const d = Math.sqrt(dx * dx + dy * dy + dz * dz);
      e -= (b.mass * bodies[j].mass) / d;
    }
  }
  return e;
}

const steps = process.argv[2] ? parseInt(process.argv[2], 10) : 8000;
const t = performance.now();
const bodies = makeBodies();
const n = bodies.length;

// Offset the sun so total momentum is zero.
let px = 0.0;
let py = 0.0;
let pz = 0.0;
for (const b of bodies) {
  px += b.vx * b.mass;
  py += b.vy * b.mass;
  pz += b.vz * b.mass;
}
const solarMass = bodies[0].mass;
bodies[0].vx = -px / solarMass;
bodies[0].vy = -py / solarMass;
bodies[0].vz = -pz / solarMass;

const e0 = energy(bodies);
const dt = 0.01;
for (let s = 0; s < steps; s++) {
  for (let i = 0; i < n; i++) {
    for (let j = i + 1; j < n; j++) {
      const dx = bodies[i].x - bodies[j].x;
      const dy = bodies[i].y - bodies[j].y;
      const dz = bodies[i].z - bodies[j].z;
      const d2 = dx * dx + dy * dy + dz * dz;
      const mag = dt / (d2 * Math.sqrt(d2));
      const mi = bodies[i].mass * mag;
      const mj = bodies[j].mass * mag;
      bodies[i].vx -= dx * mj;
      bodies[i].vy -= dy * mj;
      bodies[i].vz -= dz * mj;
      bodies[j].vx += dx * mi;
      bodies[j].vy += dy * mi;
      bodies[j].vz += dz * mi;
    }
  }
  for (let k = 0; k < n; k++) {
    const vx = bodies[k].vx;
    const vy = bodies[k].vy;
    const vz = bodies[k].vz;
    bodies[k].x += dt * vx;
    bodies[k].y += dt * vy;
    bodies[k].z += dt * vz;
  }
}
const e1 = energy(bodies);
const ns = Math.round((performance.now() - t) * 1e6);
console.log(`start ${Math.round(e0 * 1e9)} end ${Math.round(e1 * 1e9)}`);
console.error(`COMPUTE_NS ${ns}`);
