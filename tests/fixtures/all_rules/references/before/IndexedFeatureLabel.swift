import FeatureBadges
import FeatureLabels

internal func indexedFeatureLabel(_ identifier: Int) -> String {
    FeatureLabels.defaultLabel()
}

internal func indexedFeatureLabel(_ identifier: String) -> String {
    identifier
}
