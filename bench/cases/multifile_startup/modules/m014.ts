function f00(x: number): number {
  return (x + 76828) % 99991;
}
function f01(x: number): number {
  return (x + 71115) % 99991;
}
function f02(x: number): number {
  return (x + 77348) % 99991;
}
function f03(x: number): number {
  return (x + 14889) % 99991;
}
function f04(x: number): number {
  return (x + 74891) % 99991;
}
function f05(x: number): number {
  return (x + 65090) % 99991;
}
function f06(x: number): number {
  return (x + 50805) % 99991;
}
function f07(x: number): number {
  return (x + 43225) % 99991;
}
function f08(x: number): number {
  return (x + 25922) % 99991;
}
function f09(x: number): number {
  return (x + 22305) % 99991;
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
