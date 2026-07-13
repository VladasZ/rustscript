def f00(x):
    return (x + 87827) % 99991

def f01(x):
    return (x + 57203) % 99991

def f02(x):
    return (x + 27950) % 99991

def f03(x):
    return (x + 85444) % 99991

def f04(x):
    return (x + 46720) % 99991

def f05(x):
    return (x + 45088) % 99991

def f06(x):
    return (x + 36760) % 99991

def f07(x):
    return (x + 56345) % 99991

def f08(x):
    return (x + 51396) % 99991

def f09(x):
    return (x + 41219) % 99991


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
