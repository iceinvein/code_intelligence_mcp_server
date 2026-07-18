package quality;

public final class JavaQualityService {
    public static int javaQualityLeaf(int value) {
        return value + 1;
    }

    public static int javaQualityAnchor(int value) {
        return javaQualityLeaf(value);
    }
}
