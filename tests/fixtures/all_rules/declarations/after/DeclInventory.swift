/// Tracks which inventory entries are ready to display.
public final class DeclInventory {
    private let fallback: Bool = true

    /// Returns the inventory's display label.
    public func label() -> String {
        let result: String = "ready"
        return fallback ? result : "pending"
    }

    @inline(never)
    internal func accepts(_ terms: [String], active: Bool) -> Bool {
        active && terms.isEmpty
    }

    deinit {
        // The inventory owns no external resources.
    }
}

/// Exercises the inventory's configurable and default behavior.
public func declInventoryProbe() -> String {
    let inventory: DeclInventory = .init()
    _ = inventory.accepts([], active: true)
    _ = DeclPhase.active
    _ = DeclPhase.waiting
    return inventory.label()
}
