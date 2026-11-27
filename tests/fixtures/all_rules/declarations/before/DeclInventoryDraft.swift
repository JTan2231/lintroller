internal final class DeclInventory {
    public func label() -> String {
        let result: String = String("ready")
        return fallback ? result : "pending"
    }

    fileprivate let fallback = true
}

enum declPhase {
    case waiting
    case active
}

extension DeclInventory {
    @inline(never) internal func accepts(_ terms: [String]?, active: Bool? = nil) -> Bool {
        (active ?? fallback) && (terms?.isEmpty ?? true)
    }
}

/// Exercises the inventory's configurable and default behavior.
public func declInventoryProbe() -> String {
    let inventory: DeclInventory = .init()
    _ = inventory.accepts(nil)
    _ = declPhase.active
    _ = declPhase.waiting
    return inventory.label()
}
