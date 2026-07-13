def f00(x):
    return (x + 32793) % 99991

def f01(x):
    return (x + 99822) % 99991

def f02(x):
    return (x + 84424) % 99991

def f03(x):
    return (x + 22850) % 99991

def f04(x):
    return (x + 62014) % 99991

def f05(x):
    return (x + 33883) % 99991

def f06(x):
    return (x + 73013) % 99991

def f07(x):
    return (x + 56251) % 99991

def f08(x):
    return (x + 87886) % 99991

def f09(x):
    return (x + 14435) % 99991


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
