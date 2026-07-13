function f00(x: number): number {
  return (x + 80716) % 99991;
}
function f01(x: number): number {
  return (x + 19990) % 99991;
}
function f02(x: number): number {
  return (x + 25904) % 99991;
}
function f03(x: number): number {
  return (x + 30350) % 99991;
}
function f04(x: number): number {
  return (x + 59353) % 99991;
}
function f05(x: number): number {
  return (x + 73662) % 99991;
}
function f06(x: number): number {
  return (x + 24294) % 99991;
}
function f07(x: number): number {
  return (x + 68806) % 99991;
}
function f08(x: number): number {
  return (x + 59364) % 99991;
}
function f09(x: number): number {
  return (x + 24023) % 99991;
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
