package quality

func GoQualityLeaf(value int) int {
	return value + 1
}

func GoQualityAnchor(value int) int {
	return GoQualityLeaf(value)
}
