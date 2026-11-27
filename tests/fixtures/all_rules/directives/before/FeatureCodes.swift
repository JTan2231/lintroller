// swiftlint:disable implicit_return
/// Indicates whether the reserved codes are active.
public func featureCodesAreActive() -> Bool {
    return true
}

// swiftlint:disable:next line_length
// These two codes are reserved.
/// Returns whether the supplied code is reserved.
public func isReservedFeatureCode(_ code: Int) -> Bool {
    // swiftlint:disable:next no_magic_numbers
    let reserved: Bool = code == 17

    return reserved || code == 19
}
