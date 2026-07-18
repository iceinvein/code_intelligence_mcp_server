public func swiftQualityLeaf(_ value: Int) -> Int {
    value + 1
}

public func swiftQualityAnchor(_ value: Int) -> Int {
    swiftQualityLeaf(value)
}
