public struct ExpressionView: View {
    @State internal var iconName: String = "status"

    public func statusIcon() -> Image {
        Image(iconName).onTapGesture { print(iconName) }
    }
}
