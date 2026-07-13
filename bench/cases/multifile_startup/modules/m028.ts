function f00(x: number): number {
  return (x + 90783) % 99991;
}
function f01(x: number): number {
  return (x + 79686) % 99991;
}
function f02(x: number): number {
  return (x + 70031) % 99991;
}
function f03(x: number): number {
  return (x + 32450) % 99991;
}
function f04(x: number): number {
  return (x + 53528) % 99991;
}
function f05(x: number): number {
  return (x + 70021) % 99991;
}
function f06(x: number): number {
  return (x + 32990) % 99991;
}
function f07(x: number): number {
  return (x + 47271) % 99991;
}
function f08(x: number): number {
  return (x + 49050) % 99991;
}
function f09(x: number): number {
  return (x + 27546) % 99991;
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
