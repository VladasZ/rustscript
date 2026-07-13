def f00(x):
    return (x + 67134) % 99991

def f01(x):
    return (x + 2267) % 99991

def f02(x):
    return (x + 33215) % 99991

def f03(x):
    return (x + 46134) % 99991

def f04(x):
    return (x + 67838) % 99991

def f05(x):
    return (x + 90202) % 99991

def f06(x):
    return (x + 86340) % 99991

def f07(x):
    return (x + 85869) % 99991

def f08(x):
    return (x + 6439) % 99991

def f09(x):
    return (x + 69528) % 99991


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
