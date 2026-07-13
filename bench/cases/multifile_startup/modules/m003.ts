function f00(x: number): number {
  return (x + 32793) % 99991;
}
function f01(x: number): number {
  return (x + 99822) % 99991;
}
function f02(x: number): number {
  return (x + 84424) % 99991;
}
function f03(x: number): number {
  return (x + 22850) % 99991;
}
function f04(x: number): number {
  return (x + 62014) % 99991;
}
function f05(x: number): number {
  return (x + 33883) % 99991;
}
function f06(x: number): number {
  return (x + 73013) % 99991;
}
function f07(x: number): number {
  return (x + 56251) % 99991;
}
function f08(x: number): number {
  return (x + 87886) % 99991;
}
function f09(x: number): number {
  return (x + 14435) % 99991;
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
