def f00(x):
    return (x + 5860) % 99991

def f01(x):
    return (x + 86122) % 99991

def f02(x):
    return (x + 39353) % 99991

def f03(x):
    return (x + 9415) % 99991

def f04(x):
    return (x + 79900) % 99991

def f05(x):
    return (x + 23265) % 99991

def f06(x):
    return (x + 44317) % 99991

def f07(x):
    return (x + 23131) % 99991

def f08(x):
    return (x + 70529) % 99991

def f09(x):
    return (x + 73420) % 99991


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
