def f00(x):
    return (x + 25421) % 99991

def f01(x):
    return (x + 80050) % 99991

def f02(x):
    return (x + 39888) % 99991

def f03(x):
    return (x + 77291) % 99991

def f04(x):
    return (x + 66897) % 99991

def f05(x):
    return (x + 88311) % 99991

def f06(x):
    return (x + 68640) % 99991

def f07(x):
    return (x + 20393) % 99991

def f08(x):
    return (x + 10896) % 99991

def f09(x):
    return (x + 14560) % 99991


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
