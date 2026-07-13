def f00(x):
    return (x + 80716) % 99991

def f01(x):
    return (x + 19990) % 99991

def f02(x):
    return (x + 25904) % 99991

def f03(x):
    return (x + 30350) % 99991

def f04(x):
    return (x + 59353) % 99991

def f05(x):
    return (x + 73662) % 99991

def f06(x):
    return (x + 24294) % 99991

def f07(x):
    return (x + 68806) % 99991

def f08(x):
    return (x + 59364) % 99991

def f09(x):
    return (x + 24023) % 99991


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
