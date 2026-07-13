function f00(x: number): number {
  return (x + 83172) % 99991;
}
function f01(x: number): number {
  return (x + 47719) % 99991;
}
function f02(x: number): number {
  return (x + 42046) % 99991;
}
function f03(x: number): number {
  return (x + 5308) % 99991;
}
function f04(x: number): number {
  return (x + 67162) % 99991;
}
function f05(x: number): number {
  return (x + 116) % 99991;
}
function f06(x: number): number {
  return (x + 60694) % 99991;
}
function f07(x: number): number {
  return (x + 47203) % 99991;
}
function f08(x: number): number {
  return (x + 89492) % 99991;
}
function f09(x: number): number {
  return (x + 6454) % 99991;
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
