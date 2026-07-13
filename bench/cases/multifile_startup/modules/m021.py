def f00(x):
    return (x + 5593) % 99991

def f01(x):
    return (x + 96422) % 99991

def f02(x):
    return (x + 75412) % 99991

def f03(x):
    return (x + 12821) % 99991

def f04(x):
    return (x + 25617) % 99991

def f05(x):
    return (x + 3522) % 99991

def f06(x):
    return (x + 76687) % 99991

def f07(x):
    return (x + 54472) % 99991

def f08(x):
    return (x + 52807) % 99991

def f09(x):
    return (x + 25526) % 99991


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
