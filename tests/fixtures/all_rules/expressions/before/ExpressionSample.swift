/// A sample count.
public struct ExpressionSample {
    /// The measured count.
    public let count: Int

    internal init(count: Int) {
        self.count = count
    }

    /// Creates an empty sample.
    public static func baseline() -> Self {
        ExpressionSample(count: 0)
    }
}
