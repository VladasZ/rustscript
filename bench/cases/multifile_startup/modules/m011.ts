function f00(x: number): number {
  return (x + 25421) % 99991;
}
function f01(x: number): number {
  return (x + 80050) % 99991;
}
function f02(x: number): number {
  return (x + 39888) % 99991;
}
function f03(x: number): number {
  return (x + 77291) % 99991;
}
function f04(x: number): number {
  return (x + 66897) % 99991;
}
function f05(x: number): number {
  return (x + 88311) % 99991;
}
function f06(x: number): number {
  return (x + 68640) % 99991;
}
function f07(x: number): number {
  return (x + 20393) % 99991;
}
function f08(x: number): number {
  return (x + 10896) % 99991;
}
function f09(x: number): number {
  return (x + 14560) % 99991;
}

export function apply(x: number): number {
  x = f00(x);
  x = f01(x);
  x = f02(x);
  x = f03(x);
  x = f04(x);
  x = f05(x);
  x = f06(x);
  x = f07(x);
  x = f08(x);
  x = f09(x);
  return x;
}
