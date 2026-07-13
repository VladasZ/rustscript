function f00(x: number): number {
  return (x + 18760) % 99991;
}
function f01(x: number): number {
  return (x + 62977) % 99991;
}
function f02(x: number): number {
  return (x + 39198) % 99991;
}
function f03(x: number): number {
  return (x + 38146) % 99991;
}
function f04(x: number): number {
  return (x + 37268) % 99991;
}
function f05(x: number): number {
  return (x + 47715) % 99991;
}
function f06(x: number): number {
  return (x + 32605) % 99991;
}
function f07(x: number): number {
  return (x + 78297) % 99991;
}
function f08(x: number): number {
  return (x + 61225) % 99991;
}
function f09(x: number): number {
  return (x + 5308) % 99991;
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
