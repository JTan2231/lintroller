/// Formats a collection for display and returns the transformed values.
public func layoutFormatting(_ enabled: Bool, input: [Int]) -> [Int] {
    print(layoutCapacity)
    if enabled {
        return input
    }
    let bounds: [Int] = [
        0,
        1
    ]
    print(
        enabled,
        input,
        bounds
    )
    _ = input.sorted()
        .reversed()
        .reversed()
    switch enabled {
    case true:
        print(input)
    case false:
        print(bounds)
    }
    let transformed: [Int] = input.map { element in
        element + 1
    }
    return transformed
}

/// Capacity used for the layout preview.
internal let layoutCapacity: Int = 1_000
