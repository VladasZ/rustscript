def f00(x):
    return (x + 64041) % 99991

def f01(x):
    return (x + 40396) % 99991

def f02(x):
    return (x + 10300) % 99991

def f03(x):
    return (x + 69472) % 99991

def f04(x):
    return (x + 52992) % 99991

def f05(x):
    return (x + 58060) % 99991

def f06(x):
    return (x + 22802) % 99991

def f07(x):
    return (x + 62921) % 99991

def f08(x):
    return (x + 55842) % 99991

def f09(x):
    return (x + 47087) % 99991


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
