namespace Quality;

public static class CSharpQualityService
{
    public static int CSharpQualityLeaf(int value) => value + 1;

    public static int CSharpQualityAnchor(int value) => CSharpQualityLeaf(value);
}
