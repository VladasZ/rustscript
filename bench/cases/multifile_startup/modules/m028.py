def f00(x):
    return (x + 90783) % 99991

def f01(x):
    return (x + 79686) % 99991

def f02(x):
    return (x + 70031) % 99991

def f03(x):
    return (x + 32450) % 99991

def f04(x):
    return (x + 53528) % 99991

def f05(x):
    return (x + 70021) % 99991

def f06(x):
    return (x + 32990) % 99991

def f07(x):
    return (x + 47271) % 99991

def f08(x):
    return (x + 49050) % 99991

def f09(x):
    return (x + 27546) % 99991


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
