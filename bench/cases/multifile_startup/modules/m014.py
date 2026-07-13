def f00(x):
    return (x + 76828) % 99991

def f01(x):
    return (x + 71115) % 99991

def f02(x):
    return (x + 77348) % 99991

def f03(x):
    return (x + 14889) % 99991

def f04(x):
    return (x + 74891) % 99991

def f05(x):
    return (x + 65090) % 99991

def f06(x):
    return (x + 50805) % 99991

def f07(x):
    return (x + 43225) % 99991

def f08(x):
    return (x + 25922) % 99991

def f09(x):
    return (x + 22305) % 99991


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
