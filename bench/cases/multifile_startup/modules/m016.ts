function f00(x: number): number {
  return (x + 80217) % 99991;
}
function f01(x: number): number {
  return (x + 37009) % 99991;
}
function f02(x: number): number {
  return (x + 70887) % 99991;
}
function f03(x: number): number {
  return (x + 13763) % 99991;
}
function f04(x: number): number {
  return (x + 17557) % 99991;
}
function f05(x: number): number {
  return (x + 48282) % 99991;
}
function f06(x: number): number {
  return (x + 38911) % 99991;
}
function f07(x: number): number {
  return (x + 20385) % 99991;
}
function f08(x: number): number {
  return (x + 74939) % 99991;
}
function f09(x: number): number {
  return (x + 8394) % 99991;
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
