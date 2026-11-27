// Stand-ins let this fixture compile on Swift toolchains without the SwiftUI SDK.
public protocol View {}

@propertyWrapper
public struct State<Value> {
    public var wrappedValue: Value

    public init(wrappedValue: Value) {
        self.wrappedValue = wrappedValue
    }
}

public struct Image {
    public init(_ name: String) {}

    public func accessibilityLabel(_ label: String) -> Self { self }

    public func accessibilityAddTraits(_ trait: ExpressionTrait) -> Self { self }

    public func onTapGesture(_ action: () -> Void) -> Self { self }
}

public enum ExpressionTrait {
    case isButton
}

internal func expressionCombine(_ first: () -> Int, _ second: () -> Int) -> Int {
    first() + second()
}
