/// A sample count.
public struct ExpressionSample {
    /// The measured count.
    public let count: Int

    /// Creates an empty sample.
    public static func baseline() -> Self {
        Self(count: 0)
    }
}
