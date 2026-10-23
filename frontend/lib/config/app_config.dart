/// Centralized application configuration.
/// 
/// Contains backend base URL, timeouts, OAuth configuration,
/// and client headers conforming to the backend API specification.
class AppConfig {
  /// Base URL of the Rust backend.
  /// Verified backend default port is 8080 (HOST=127.0.0.1, PORT=8080 in .env.example).
  static String backendBaseUrl = const String.fromEnvironment(
    'BACKEND_URL',
    defaultValue: 'http://127.0.0.1:8080',
  );

  /// Client identification header value for audit and routing
  static const String clientType = 'flutter';

  /// Google OAuth Web Client ID.
  /// Configure this with your actual Web Client ID from Google Cloud Console.
  static String googleClientId = const String.fromEnvironment(
    'GOOGLE_CLIENT_ID',
    defaultValue: '164223517067-s37obp9gkjdp0lnjpifpdv6239belrjq.apps.googleusercontent.com',
  );

  /// Check if Google Client ID is configured.
  static bool get hasGoogleClientId => googleClientId.trim().isNotEmpty;

  /// Google Drive AppData scope used by the backend for exclusive data storage.
  static const String googleDriveScope =
      'https://www.googleapis.com/auth/drive.appdata';

  /// Request timeout in seconds
  static const Duration requestTimeout = Duration(seconds: 30);

  /// Stream connect timeout
  static const Duration streamConnectTimeout = Duration(seconds: 15);
}
