def f00(x):
    return (x + 97557) % 99991

def f01(x):
    return (x + 16685) % 99991

def f02(x):
    return (x + 59373) % 99991

def f03(x):
    return (x + 53691) % 99991

def f04(x):
    return (x + 93356) % 99991

def f05(x):
    return (x + 89092) % 99991

def f06(x):
    return (x + 65690) % 99991

def f07(x):
    return (x + 97668) % 99991

def f08(x):
    return (x + 93440) % 99991

def f09(x):
    return (x + 60187) % 99991


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
