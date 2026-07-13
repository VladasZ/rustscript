def f00(x):
    return (x + 83172) % 99991

def f01(x):
    return (x + 47719) % 99991

def f02(x):
    return (x + 42046) % 99991

def f03(x):
    return (x + 5308) % 99991

def f04(x):
    return (x + 67162) % 99991

def f05(x):
    return (x + 116) % 99991

def f06(x):
    return (x + 60694) % 99991

def f07(x):
    return (x + 47203) % 99991

def f08(x):
    return (x + 89492) % 99991

def f09(x):
    return (x + 6454) % 99991


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
