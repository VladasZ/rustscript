function f00(x: number): number {
  return (x + 68919) % 99991;
}
function f01(x: number): number {
  return (x + 72621) % 99991;
}
function f02(x: number): number {
  return (x + 68690) % 99991;
}
function f03(x: number): number {
  return (x + 62028) % 99991;
}
function f04(x: number): number {
  return (x + 55401) % 99991;
}
function f05(x: number): number {
  return (x + 42740) % 99991;
}
function f06(x: number): number {
  return (x + 69068) % 99991;
}
function f07(x: number): number {
  return (x + 87191) % 99991;
}
function f08(x: number): number {
  return (x + 75313) % 99991;
}
function f09(x: number): number {
  return (x + 72726) % 99991;
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
