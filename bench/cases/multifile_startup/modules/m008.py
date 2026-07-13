def f00(x):
    return (x + 68919) % 99991

def f01(x):
    return (x + 72621) % 99991

def f02(x):
    return (x + 68690) % 99991

def f03(x):
    return (x + 62028) % 99991

def f04(x):
    return (x + 55401) % 99991

def f05(x):
    return (x + 42740) % 99991

def f06(x):
    return (x + 69068) % 99991

def f07(x):
    return (x + 87191) % 99991

def f08(x):
    return (x + 75313) % 99991

def f09(x):
    return (x + 72726) % 99991


def apply(x):
    x = f00(x)
    x = f01(x)
    x = f02(x)
    x = f03(x)
    x = f04(x)
    x = f05(x)
    x = f06(x)
    x = f07(x)
    x = f08(x)
    x = f09(x)
    return x
