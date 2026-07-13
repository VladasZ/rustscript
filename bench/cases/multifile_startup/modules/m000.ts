function f00(x: number): number {
  return (x + 8958) % 99991;
}
function f01(x: number): number {
  return (x + 73354) % 99991;
}
function f02(x: number): number {
  return (x + 15943) % 99991;
}
function f03(x: number): number {
  return (x + 15416) % 99991;
}
function f04(x: number): number {
  return (x + 19843) % 99991;
}
function f05(x: number): number {
  return (x + 46081) % 99991;
}
function f06(x: number): number {
  return (x + 16617) % 99991;
}
function f07(x: number): number {
  return (x + 55612) % 99991;
}
function f08(x: number): number {
  return (x + 50163) % 99991;
}
function f09(x: number): number {
  return (x + 58450) % 99991;
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
