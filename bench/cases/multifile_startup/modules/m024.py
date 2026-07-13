def f00(x):
    return (x + 10205) % 99991

def f01(x):
    return (x + 20570) % 99991

def f02(x):
    return (x + 74845) % 99991

def f03(x):
    return (x + 85295) % 99991

def f04(x):
    return (x + 8120) % 99991

def f05(x):
    return (x + 51247) % 99991

def f06(x):
    return (x + 45785) % 99991

def f07(x):
    return (x + 82580) % 99991

def f08(x):
    return (x + 87750) % 99991

def f09(x):
    return (x + 33609) % 99991


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
