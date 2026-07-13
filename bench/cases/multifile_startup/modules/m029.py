def f00(x):
    return (x + 94816) % 99991

def f01(x):
    return (x + 98618) % 99991

def f02(x):
    return (x + 33487) % 99991

def f03(x):
    return (x + 67132) % 99991

def f04(x):
    return (x + 82078) % 99991

def f05(x):
    return (x + 38484) % 99991

def f06(x):
    return (x + 77336) % 99991

def f07(x):
    return (x + 81712) % 99991

def f08(x):
    return (x + 61759) % 99991

def f09(x):
    return (x + 85139) % 99991


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
