public struct ExpressionView: View {
    @State private var iconName: String = "status"

    public func statusIcon() -> Image {
        Image(iconName)
            .accessibilityLabel("Show status")
            .onTapGesture { print(iconName) }
            .accessibilityAddTraits(.isButton)
    }
}
