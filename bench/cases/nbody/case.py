import sys
import time


def make_bodies():
    pi = 3.141592653589793
    solar_mass = 4.0 * pi * pi
    days = 365.24
    return [
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, solar_mass],
        [
            4.84143144246472090,
            -1.16032004402742839,
            -0.103622044471123109,
            0.00166007664274403694 * days,
            0.00769901118419740425 * days,
            -0.0000690460016972063023 * days,
            0.000954791938424326609 * solar_mass,
        ],
        [
            8.34336671824457987,
            4.12479856412430479,
            -0.403523417114321381,
            -0.00276742510726862411 * days,
            0.00499852801234917238 * days,
            0.0000230417297573763929 * days,
            0.000285885980666130812 * solar_mass,
        ],
        [
            12.8943695621391310,
            -15.1111514016986312,
            -0.223307578892655734,
            0.00296460137564761618 * days,
            0.00237847173959480950 * days,
            -0.0000296589568540237556 * days,
            0.0000436624404335156298 * solar_mass,
        ],
        [
            15.3796971148509165,
            -25.9193146099879641,
            0.179258772950371181,
            0.00268067772490389322 * days,
            0.00162824170038242295 * days,
            -0.0000951592254519715870 * days,
            0.0000515138902046611451 * solar_mass,
        ],
    ]


X, Y, Z, VX, VY, VZ, MASS = range(7)


def energy(bodies):
    n = len(bodies)
    e = 0.0
    for i in range(n):
        b = bodies[i]
        e += 0.5 * b[MASS] * (b[VX] * b[VX] + b[VY] * b[VY] + b[VZ] * b[VZ])
        for j in range(i + 1, n):
            dx = b[X] - bodies[j][X]
            dy = b[Y] - bodies[j][Y]
            dz = b[Z] - bodies[j][Z]
            d = (dx * dx + dy * dy + dz * dz) ** 0.5
            e -= b[MASS] * bodies[j][MASS] / d
    return e


steps = int(sys.argv[1]) if len(sys.argv) > 1 else 8000
t = time.perf_counter_ns()
bodies = make_bodies()
n = len(bodies)

# Offset the sun so total momentum is zero.
px = 0.0
py = 0.0
pz = 0.0
for b in bodies:
    px += b[VX] * b[MASS]
    py += b[VY] * b[MASS]
    pz += b[VZ] * b[MASS]
solar_mass = bodies[0][MASS]
bodies[0][VX] = -px / solar_mass
bodies[0][VY] = -py / solar_mass
bodies[0][VZ] = -pz / solar_mass

e0 = energy(bodies)
dt = 0.01
for _ in range(steps):
    for i in range(n):
        for j in range(i + 1, n):
            dx = bodies[i][X] - bodies[j][X]
            dy = bodies[i][Y] - bodies[j][Y]
            dz = bodies[i][Z] - bodies[j][Z]
            d2 = dx * dx + dy * dy + dz * dz
            mag = dt / (d2 * d2 ** 0.5)
            mi = bodies[i][MASS] * mag
            mj = bodies[j][MASS] * mag
            bodies[i][VX] -= dx * mj
            bodies[i][VY] -= dy * mj
            bodies[i][VZ] -= dz * mj
            bodies[j][VX] += dx * mi
            bodies[j][VY] += dy * mi
            bodies[j][VZ] += dz * mi
    for k in range(n):
        bodies[k][X] += dt * bodies[k][VX]
        bodies[k][Y] += dt * bodies[k][VY]
        bodies[k][Z] += dt * bodies[k][VZ]
e1 = energy(bodies)
ns = time.perf_counter_ns() - t


def scaled(e):
    v = e * 1e9
    f = int(v // 1.0)
    return f + 1 if v - f >= 0.5 else f


print(f"start {scaled(e0)} end {scaled(e1)}")
print(f"COMPUTE_NS {ns}", file=sys.stderr)
