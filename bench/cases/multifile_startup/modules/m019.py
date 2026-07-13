def f00(x):
    return (x + 58286) % 99991

def f01(x):
    return (x + 54887) % 99991

def f02(x):
    return (x + 73106) % 99991

def f03(x):
    return (x + 57724) % 99991

def f04(x):
    return (x + 53809) % 99991

def f05(x):
    return (x + 10656) % 99991

def f06(x):
    return (x + 83435) % 99991

def f07(x):
    return (x + 47106) % 99991

def f08(x):
    return (x + 28473) % 99991

def f09(x):
    return (x + 45986) % 99991


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
