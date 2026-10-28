/// Centralized application configuration.
/// 
/// Contains backend base URL, timeouts, OAuth configuration,
/// and client headers conforming to the backend API specification.
class AppConfig {
  /// Base URL of the Rust backend.
  /// Configured to the verified deployed production HTTPS backend on Vercel.
  /// Can be overridden via `--dart-define=BACKEND_URL=<url>` for local dev or staging.
  static String backendBaseUrl = const String.fromEnvironment(
    'BACKEND_URL',
    defaultValue: 'https://rust-backend-fawn.vercel.app',
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
