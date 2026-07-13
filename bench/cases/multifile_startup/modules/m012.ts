function f00(x: number): number {
  return (x + 3314) % 99991;
}
function f01(x: number): number {
  return (x + 33281) % 99991;
}
function f02(x: number): number {
  return (x + 86202) % 99991;
}
function f03(x: number): number {
  return (x + 6619) % 99991;
}
function f04(x: number): number {
  return (x + 57826) % 99991;
}
function f05(x: number): number {
  return (x + 16685) % 99991;
}
function f06(x: number): number {
  return (x + 27517) % 99991;
}
function f07(x: number): number {
  return (x + 97503) % 99991;
}
function f08(x: number): number {
  return (x + 86137) % 99991;
}
function f09(x: number): number {
  return (x + 18792) % 99991;
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
