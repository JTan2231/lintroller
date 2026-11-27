/// Indicates whether the reserved codes are active.
public func featureCodesAreActive() -> Bool {
    true
}

/// Returns whether the supplied code is reserved.
public func isReservedFeatureCode(_ code: Int) -> Bool {
    let secondaryCode: Int = 19
    // swiftlint:disable:next no_magic_numbers
    let reserved: Bool = code == 17

    return reserved || code == secondaryCode
}
