int c_quality_leaf(int value) {
    return value + 1;
}

int c_quality_anchor(int value) {
    return c_quality_leaf(value);
}
