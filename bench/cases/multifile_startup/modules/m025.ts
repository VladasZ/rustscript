function f00(x: number): number {
  return (x + 87827) % 99991;
}
function f01(x: number): number {
  return (x + 57203) % 99991;
}
function f02(x: number): number {
  return (x + 27950) % 99991;
}
function f03(x: number): number {
  return (x + 85444) % 99991;
}
function f04(x: number): number {
  return (x + 46720) % 99991;
}
function f05(x: number): number {
  return (x + 45088) % 99991;
}
function f06(x: number): number {
  return (x + 36760) % 99991;
}
function f07(x: number): number {
  return (x + 56345) % 99991;
}
function f08(x: number): number {
  return (x + 51396) % 99991;
}
function f09(x: number): number {
  return (x + 41219) % 99991;
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
