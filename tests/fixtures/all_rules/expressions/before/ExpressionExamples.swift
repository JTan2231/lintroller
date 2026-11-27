import Foundation

/// Counts UTF-16 code units.
public func expressionLength(_ value: NSString) -> Int {
    value.length
}

/// Adds the shipping offset to a position.
public func expressionShift(_ value: Int) -> Int {
    return value + 12
}

/// Keeps the positive elements.
public func expressionFilter(_ values: [Int]) -> [Int] {
    values.filter({
        $0 > 0
    })
}

/// Counts the characters in each word.
public func expressionCounts(_ words: [String]) -> [Int] {
    words.map { $0.count }
}

/// Combines the two seeds.
public func expressionSeeds() -> Int {
    expressionCombine({ 0 }) { 1 }
}

/// Allows the operation when both conditions are true.
public func expressionAllows(_ allowed: Bool, _ ready: Bool) -> Bool {
    if allowed && ready {
        return true
    }
    return false
}

/// Clamps negative inputs to zero.
public func expressionClamp(_ input: Int) -> Int {
    if input < 0 {
        let input: Int = 0
        return input
    }
    return input
}

/// Executes the supplied operation.
public func expressionPerform(_ operation: () -> Void) {}

/// Sums the measurements with an optional offset.
public func expressionSum(
    offset: Int = 0,
    first: Int,
    second: Int,
    third: Int,
    fourth: Int,
    fifth: Int,
    sixth: Int
) -> Int {
    offset + first + second + third + fourth + fifth + sixth
}

/// Returns true when a negative sign is present.
public func expressionIsNegative(_ sign: FloatingPointSign?) -> Bool {
    switch sign {
    case .minus?:
        return true
    default:
        return false
    }
}
