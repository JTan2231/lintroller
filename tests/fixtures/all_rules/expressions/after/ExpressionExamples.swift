/// Counts UTF-16 code units.
public func expressionLength(_ value: String) -> Int {
    value.utf16.count
}

/// Adds the shipping offset to a position.
public func expressionShift(_ value: Int) -> Int {
    let shippingOffset: Int = 12
    return value + shippingOffset
}

/// Keeps the positive elements.
public func expressionFilter(_ values: [Int]) -> [Int] {
    values.filter { element in
        element > 0
    }
}

/// Counts the characters in each word.
public func expressionCounts(_ words: [String]) -> [Int] {
    words.map(\.count)
}

/// Combines the two seeds.
public func expressionSeeds() -> Int {
    expressionCombine({ 0 }, { 1 })
}

/// Allows the operation when both conditions are true.
public func expressionAllows(_ allowed: Bool, _ ready: Bool) -> Bool {
    if allowed, ready {
        return true
    }
    return false
}

/// Clamps negative inputs to zero.
public func expressionClamp(_ input: Int) -> Int {
    if input < 0 {
        let minimum: Int = 0
        return minimum
    }
    return input
}

/// Executes the supplied operation.
public func expressionPerform(_ operation: () -> Void) {
    operation()
}

/// Sums the measurements with an optional offset.
public func expressionSum(_ measurements: [Int], offset: Int = 0) -> Int {
    measurements.reduce(offset, +)
}

/// Returns true when a negative sign is present.
public func expressionIsNegative(_ sign: FloatingPointSign?) -> Bool {
    switch sign {
    case .minus:
        return true
    default:
        return false
    }
}
