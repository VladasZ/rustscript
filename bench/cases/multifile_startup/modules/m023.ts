function f00(x: number): number {
  return (x + 55215) % 99991;
}
function f01(x: number): number {
  return (x + 61088) % 99991;
}
function f02(x: number): number {
  return (x + 64339) % 99991;
}
function f03(x: number): number {
  return (x + 39633) % 99991;
}
function f04(x: number): number {
  return (x + 71656) % 99991;
}
function f05(x: number): number {
  return (x + 51093) % 99991;
}
function f06(x: number): number {
  return (x + 26796) % 99991;
}
function f07(x: number): number {
  return (x + 92491) % 99991;
}
function f08(x: number): number {
  return (x + 9298) % 99991;
}
function f09(x: number): number {
  return (x + 4886) % 99991;
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
