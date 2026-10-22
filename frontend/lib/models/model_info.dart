// Representation of AI models registered in the backend catalog
// and reasoning/thinking modes selectable via the Think button.

class ModelInfo {
  final String modelId;
  final String displayName;
  final String providerId;
  final List<String> supportedModalities;
  final int contextWindow;
  final int maxOutputTokens;
  final bool streamingSupported;
  final bool toolCallingSupported;
  final String speedTier;
  final String costTier;
  final bool available;
  final List<String> capabilities;
  final String? lastHealthCheck;

  ModelInfo({
    required this.modelId,
    required this.displayName,
    required this.providerId,
    required this.supportedModalities,
    required this.contextWindow,
    required this.maxOutputTokens,
    required this.streamingSupported,
    required this.toolCallingSupported,
    required this.speedTier,
    required this.costTier,
    required this.available,
    required this.capabilities,
    this.lastHealthCheck,
  });

  factory ModelInfo.fromJson(Map<String, dynamic> json) {
    return ModelInfo(
      modelId: json['model_id'] as String? ?? '',
      displayName: json['display_name'] as String? ?? '',
      providerId: json['provider_id'] as String? ?? '',
      supportedModalities: (json['supported_modalities'] as List<dynamic>?)
              ?.map((e) => e.toString())
              .toList() ??
          const [],
      contextWindow: json['context_window'] as int? ?? 4096,
      maxOutputTokens: json['max_output_tokens'] as int? ?? 2048,
      streamingSupported: json['streaming_supported'] as bool? ?? true,
      toolCallingSupported: json['tool_calling_supported'] as bool? ?? false,
      speedTier: json['speed_tier'] as String? ?? 'balanced',
      costTier: json['cost_tier'] as String? ?? 'free-tier',
      available: json['available'] as bool? ?? true,
      capabilities: (json['capabilities'] as List<dynamic>?)
              ?.map((e) => e.toString())
              .toList() ??
          const [],
      lastHealthCheck: json['last_health_check'] as String?,
    );
  }
}

/// Reasoning / Thinking mode options for the Think button in the composer.
class ThinkMode {
  final String id;
  final String label;
  final String description;
  final String routingMode; // "auto" or "manual"
  final String? preferredSpeedTier;

  const ThinkMode({
    required this.id,
    required this.label,
    required this.description,
    this.routingMode = 'auto',
    this.preferredSpeedTier,
  });

  static const ThinkMode auto = ThinkMode(
    id: 'auto',
    label: 'Auto Thinking',
    description: 'Intelligently analyzes query complexity and chooses best model',
    routingMode: 'auto',
  );

  static const ThinkMode fast = ThinkMode(
    id: 'fast',
    label: 'Fast Speed',
    description: 'Optimized for quick, low-latency conversational responses',
    routingMode: 'auto',
    preferredSpeedTier: 'fast',
  );

  static const ThinkMode deep = ThinkMode(
    id: 'deep',
    label: 'Deep Reasoning',
    description: 'Multi-step cognitive breakdown for complex technical tasks',
    routingMode: 'auto',
    preferredSpeedTier: 'advanced',
  );

  static const ThinkMode standard = ThinkMode(
    id: 'standard',
    label: 'Standard (Off)',
    description: 'Standard prompt routing with baseline inference',
    routingMode: 'auto',
  );

  static const List<ThinkMode> supportedModes = [
    auto,
    fast,
    deep,
    standard,
  ];
}
