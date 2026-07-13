function f00(x: number): number {
  return (x + 64041) % 99991;
}
function f01(x: number): number {
  return (x + 40396) % 99991;
}
function f02(x: number): number {
  return (x + 10300) % 99991;
}
function f03(x: number): number {
  return (x + 69472) % 99991;
}
function f04(x: number): number {
  return (x + 52992) % 99991;
}
function f05(x: number): number {
  return (x + 58060) % 99991;
}
function f06(x: number): number {
  return (x + 22802) % 99991;
}
function f07(x: number): number {
  return (x + 62921) % 99991;
}
function f08(x: number): number {
  return (x + 55842) % 99991;
}
function f09(x: number): number {
  return (x + 47087) % 99991;
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
