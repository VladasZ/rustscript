function f00(x: number): number {
  return (x + 5860) % 99991;
}
function f01(x: number): number {
  return (x + 86122) % 99991;
}
function f02(x: number): number {
  return (x + 39353) % 99991;
}
function f03(x: number): number {
  return (x + 9415) % 99991;
}
function f04(x: number): number {
  return (x + 79900) % 99991;
}
function f05(x: number): number {
  return (x + 23265) % 99991;
}
function f06(x: number): number {
  return (x + 44317) % 99991;
}
function f07(x: number): number {
  return (x + 23131) % 99991;
}
function f08(x: number): number {
  return (x + 70529) % 99991;
}
function f09(x: number): number {
  return (x + 73420) % 99991;
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
