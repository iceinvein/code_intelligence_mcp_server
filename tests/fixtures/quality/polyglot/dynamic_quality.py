def dynamic_quality_leaf(value: int) -> int:
    return value + 1


def dynamic_quality_dispatch(name: str, value: int) -> int:
    target = globals()[name]
    return target(value)
