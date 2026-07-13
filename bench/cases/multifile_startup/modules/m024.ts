function f00(x: number): number {
  return (x + 10205) % 99991;
}
function f01(x: number): number {
  return (x + 20570) % 99991;
}
function f02(x: number): number {
  return (x + 74845) % 99991;
}
function f03(x: number): number {
  return (x + 85295) % 99991;
}
function f04(x: number): number {
  return (x + 8120) % 99991;
}
function f05(x: number): number {
  return (x + 51247) % 99991;
}
function f06(x: number): number {
  return (x + 45785) % 99991;
}
function f07(x: number): number {
  return (x + 82580) % 99991;
}
function f08(x: number): number {
  return (x + 87750) % 99991;
}
function f09(x: number): number {
  return (x + 33609) % 99991;
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
