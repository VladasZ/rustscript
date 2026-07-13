function f00(x: number): number {
  return (x + 58286) % 99991;
}
function f01(x: number): number {
  return (x + 54887) % 99991;
}
function f02(x: number): number {
  return (x + 73106) % 99991;
}
function f03(x: number): number {
  return (x + 57724) % 99991;
}
function f04(x: number): number {
  return (x + 53809) % 99991;
}
function f05(x: number): number {
  return (x + 10656) % 99991;
}
function f06(x: number): number {
  return (x + 83435) % 99991;
}
function f07(x: number): number {
  return (x + 47106) % 99991;
}
function f08(x: number): number {
  return (x + 28473) % 99991;
}
function f09(x: number): number {
  return (x + 45986) % 99991;
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
