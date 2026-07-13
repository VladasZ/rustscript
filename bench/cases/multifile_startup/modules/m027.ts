function f00(x: number): number {
  return (x + 14415) % 99991;
}
function f01(x: number): number {
  return (x + 58432) % 99991;
}
function f02(x: number): number {
  return (x + 11288) % 99991;
}
function f03(x: number): number {
  return (x + 2785) % 99991;
}
function f04(x: number): number {
  return (x + 92570) % 99991;
}
function f05(x: number): number {
  return (x + 25864) % 99991;
}
function f06(x: number): number {
  return (x + 17709) % 99991;
}
function f07(x: number): number {
  return (x + 84845) % 99991;
}
function f08(x: number): number {
  return (x + 66446) % 99991;
}
function f09(x: number): number {
  return (x + 62055) % 99991;
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
