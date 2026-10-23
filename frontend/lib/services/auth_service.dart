import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:google_sign_in/google_sign_in.dart';
import '../config/app_config.dart';
import '../models/auth_user.dart';

/// Authentication service for Google Sign-In and Google Drive authorization.
/// 
/// Strictly adheres to security rules:
/// - In-memory token management (no secrets stored in localStorage or persistent storage).
/// - Initiates genuine Google OAuth / OIDC flow for Web (Edge) and mobile.
/// - Separates identity authentication (`idToken`) from Google Drive scope authorization (`accessToken`).
/// - Isolates developer/offline simulated sessions so they cannot masquerade as real Google users.
/// - Emits user changes to listeners.
class AuthService extends ChangeNotifier {
  AuthUser? _currentUser;
  GoogleSignInAccount? _googleAccount;
  bool _isLoading = false;
  String? _lastError;
  bool _isInitialized = false;
  bool _isInitializing = false;
  Future<void>? _initFuture;
  StreamSubscription<GoogleSignInAuthenticationEvent>? _authEventsSub;

  AuthUser? get currentUser => _currentUser;
  bool get isAuthenticated => _currentUser != null;
  bool get isLoading => _isLoading;
  String? get lastError => _lastError;
  bool get isMockSession => _currentUser?.isMock ?? false;
  bool get isInitialized => _isInitialized;
  bool get isInitializing => _isInitializing;

  /// Ensure GoogleSignIn plugin is initialized with current Web Client ID.
  Future<void> ensureInitialized() {
    if (_isInitialized) return Future.value();
    if (_initFuture != null) return _initFuture!;

    _isInitializing = true;
    _initFuture = _doInitialize();
    return _initFuture!;
  }

  Future<void> _doInitialize() async {
    try {
      final clientId = AppConfig.googleClientId.trim();

      // On Web, Google Identity Services requires serverClientId to be null.
      await GoogleSignIn.instance.initialize(
        clientId: clientId.isNotEmpty ? clientId : null,
        serverClientId: kIsWeb ? null : (clientId.isNotEmpty ? clientId : null),
      ).timeout(
        const Duration(seconds: 8),
        onTimeout: () {
          throw TimeoutException('Google Sign-In initialization timed out.');
        },
      );

      _authEventsSub ??= GoogleSignIn.instance.authenticationEvents.listen(
        _handleAuthenticationEvent,
        onError: (e) {
          _lastError = 'Authentication error: $e';
          _isLoading = false;
          notifyListeners();
        },
      );

      _isInitialized = true;

      // On Web, attempt lightweight authentication (One Tap / silent session) if user previously signed in
      if (kIsWeb) {
        GoogleSignIn.instance.attemptLightweightAuthentication();
      }
    } catch (e) {
      if (kDebugMode) {
        print('GoogleSignIn initialize notice: $e');
      }
      _lastError = 'Google Sign-In setup failed: $e';
    } finally {
      _isInitializing = false;
      notifyListeners();
    }
  }

  Future<void> _handleAuthenticationEvent(GoogleSignInAuthenticationEvent event) async {
    switch (event) {
      case GoogleSignInAuthenticationEventSignIn():
        await _processAccount(event.user);
      case GoogleSignInAuthenticationEventSignOut():
        _currentUser = null;
        _googleAccount = null;
        _isLoading = false;
        notifyListeners();
    }
  }

  Future<bool> _processAccount(GoogleSignInAccount account) async {
    final GoogleSignInAuthentication auth = account.authentication;
    final String? idToken = auth.idToken;

    if (idToken == null || idToken.trim().isEmpty) {
      _lastError = 'Google Sign-In succeeded, but no ID token was returned. '
          'Verify that GOOGLE_CLIENT_ID is a Web application client ID in Google Cloud Console '
          'and that your browser origin is added to Authorized JavaScript origins.';
      _isLoading = false;
      notifyListeners();
      return false;
    }

    _googleAccount = account;
    _currentUser = AuthUser(
      id: 'google:${account.id}',
      name: account.displayName ?? (account.email.isNotEmpty ? account.email.split('@').first : 'Google User'),
      email: account.email,
      photoUrl: account.photoUrl,
      idToken: idToken.trim(),
      driveAccessToken: null, // Drive authorization is requested explicitly
      isMock: false,
    );

    _isLoading = false;
    _lastError = null;
    notifyListeners();

    if (kDebugMode) {
      final email = account.email;
      final redactedEmail = email.contains('@')
          ? '${email.split('@').first.substring(0, email.split('@').first.length > 2 ? 2 : 1)}***@${email.split('@').last}'
          : '***';
      final accId = account.id;
      final redactedId = accId.length > 6
          ? '${accId.substring(0, 3)}***${accId.substring(accId.length - 3)}'
          : '***';
      print('[RUNTIME-TRACE][FRONTEND] _processAccount: Google account authenticated: id=google:$redactedId, email=$redactedEmail. ID token len: ${idToken.trim().length}. Now calling authorizeDrive(silent: true)...');
    }

    // Attempt immediate Drive authorization for seamless experience if client supports it
    final silentSuccess = await authorizeDrive(silent: true);
    if (kDebugMode) {
      print('[RUNTIME-TRACE][FRONTEND] authorizeDrive(silent: true) returned: $silentSuccess. currentUser.hasDriveToken: ${_currentUser?.hasDriveToken}');
    }

    return true;
  }

  /// Initiate genuine Google Sign-In.
  /// 
  /// On Web (Edge), Google Identity Services requires user interaction via the rendered
  /// button (`renderButton`) and explicitly forbids `authenticate()`.
  /// On mobile/desktop, native `authenticate()` is called.
  /// 
  /// Returns `true` if immediate authentication succeeds, `false` otherwise.
  Future<bool> signInWithGoogle() async {
    _isLoading = true;
    _lastError = null;
    notifyListeners();

    try {
      // 1. Verify that a Web Client ID is configured
      if (!AppConfig.hasGoogleClientId) {
        _lastError = 'Google OAuth Web Client ID is not configured. '
            'Please pass --dart-define=GOOGLE_CLIENT_ID=<id> or enter your Web Client ID in Token Settings.';
        _isLoading = false;
        notifyListeners();
        return false;
      }

      // 2. Initialize GoogleSignIn instance
      await ensureInitialized();

      // 3. Initiate authentication according to platform capabilities
      if (!kIsWeb) {
        // Native mobile/desktop platforms support authenticate()
        final GoogleSignInAccount account = await GoogleSignIn.instance.authenticate();
        return await _processAccount(account);
      } else {
        // Web: Google Identity Services prohibits imperative authenticate() calls.
        // Sign-in is handled through the rendered GSI button (renderButton) or One Tap.
        GoogleSignIn.instance.attemptLightweightAuthentication();
        _isLoading = false;
        notifyListeners();
        return false;
      }
    } catch (e) {
      final msg = e.toString().toLowerCase();
      if (msg.contains('popup_closed') || msg.contains('cancel') || msg.contains('abort')) {
        _lastError = null; // Clean cancel, do not treat as fatal error
      } else {
        _lastError = 'Google Sign-In failed: $e';
      }
      _isLoading = false;
      notifyListeners();
      return false;
    }
  }

  /// Request Google Drive AppData scope authorization (`https://www.googleapis.com/auth/drive.appdata`).
  /// 
  /// Uses Google Identity Services scope authorization to obtain an access token
  /// strictly for Google Drive persistence (`X-Drive-Token: <token>`).
  Future<bool> authorizeDrive({bool silent = false}) async {
    if (_currentUser == null) return false;

    // For mock test sessions, Drive authorization is simulated
    if (_currentUser!.isMock) {
      if (_currentUser!.driveAccessToken == null) {
        _currentUser = _currentUser!.copyWith(
          driveAccessToken: 'mock_drive_token_${DateTime.now().millisecondsSinceEpoch}',
        );
        notifyListeners();
      }
      return true;
    }

    final account = _googleAccount;
    if (account == null) {
      if (!silent) {
        _lastError = 'No active Google account found to authorize Drive access.';
        notifyListeners();
      }
      return false;
    }

    try {
      final client = account.authorizationClient;
      final scopes = [AppConfig.googleDriveScope];

      String? accessToken;
      if (silent) {
        // Check if user has already granted Drive scope in this session
        final existingAuth = await client.authorizationForScopes(scopes);
        accessToken = existingAuth?.accessToken;
        if (kDebugMode) {
          print('[RUNTIME-TRACE][FRONTEND] authorizeDrive(silent: true): client.authorizationForScopes returned accessToken present: ${accessToken != null && accessToken.trim().isNotEmpty} (len: ${accessToken?.trim().length ?? 0})');
        }
      } else {
        // Interactively request Drive scope from Google Identity Services token client
        final newAuth = await client.authorizeScopes(scopes);
        accessToken = newAuth.accessToken;
        if (kDebugMode) {
          print('[RUNTIME-TRACE][FRONTEND] authorizeDrive(interactive): client.authorizeScopes returned accessToken present: ${accessToken.trim().isNotEmpty} (len: ${accessToken.trim().length})');
        }
      }

      if (accessToken != null && accessToken.trim().isNotEmpty) {
        _currentUser = _currentUser!.copyWith(driveAccessToken: accessToken.trim());
        _lastError = null;
        notifyListeners();
        return true;
      } else {
        if (kDebugMode) {
          print('[RUNTIME-TRACE][FRONTEND] authorizeDrive(silent: $silent): No Drive access token obtained');
        }
        if (!silent) {
          _lastError = 'Google Drive authorization was denied. Conversations cannot be synced to Drive.';
          notifyListeners();
        }
        return false;
      }
    } catch (e) {
      if (!silent) {
        _lastError = 'Failed to obtain Google Drive authorization: $e';
        notifyListeners();
      }
      return false;
    }
  }

  /// Launch developer test session for automated UI verification and offline testing.
  /// 
  /// Strictly marked with [AuthUser.isMock] = true so it is never treated as a real Google account.
  Future<bool> launchDevSession({
    String? simulatedName,
    String? simulatedEmail,
    String? customIdToken,
    String? customDriveToken,
  }) async {
    _isLoading = true;
    _lastError = null;
    notifyListeners();

    final name = simulatedName ?? 'Ateeb (Test)';
    final email = simulatedEmail ?? 'ateeb.developer@gmail.com';

    _currentUser = AuthUser(
      id: 'mock_user_${DateTime.now().millisecondsSinceEpoch}',
      name: name,
      email: email,
      photoUrl: null,
      idToken: customIdToken ?? 'mock_dev_id_token_${DateTime.now().millisecondsSinceEpoch}',
      driveAccessToken: customDriveToken ?? 'mock_dev_drive_token_${DateTime.now().millisecondsSinceEpoch}',
      isMock: true,
    );

    _isLoading = false;
    notifyListeners();
    return true;
  }

  /// Sign in using explicitly provided tokens (e.g. backend PAT / API key or manual Google tokens).
  Future<bool> signInWithCustomTokens({
    required String idToken,
    String? driveToken,
    String? name,
    String? email,
  }) async {
    _isLoading = true;
    _lastError = null;
    notifyListeners();

    final cleanIdToken = idToken.trim();
    if (cleanIdToken.isEmpty) {
      _lastError = 'Token cannot be empty';
      _isLoading = false;
      notifyListeners();
      return false;
    }

    _currentUser = AuthUser(
      id: 'custom_user_${DateTime.now().millisecondsSinceEpoch}',
      name: name ?? 'Developer User',
      email: email ?? 'developer@localhost',
      photoUrl: null,
      idToken: cleanIdToken,
      driveAccessToken: driveToken?.trim().isNotEmpty == true ? driveToken!.trim() : null,
      isMock: false,
    );

    _isLoading = false;
    notifyListeners();
    return true;
  }

  /// Update the Google Drive access token (e.g. after refresh or re-authorization).
  void updateDriveToken(String driveToken) {
    if (_currentUser != null) {
      _currentUser = _currentUser!.copyWith(driveAccessToken: driveToken.trim());
      notifyListeners();
    }
  }

  /// Sign out current account and clear all in-memory credentials.
  /// Remote Drive data is never deleted.
  Future<void> signOut() async {
    try {
      if (_googleAccount != null) {
        await GoogleSignIn.instance.signOut();
      }
    } catch (_) {}

    _currentUser = null;
    _googleAccount = null;
    _lastError = null;
    notifyListeners();
  }

  /// Switch account: cleans current session state to allow logging in as another Google account.
  Future<void> switchAccount() async {
    await signOut();
  }

  @override
  void dispose() {
    _authEventsSub?.cancel();
    super.dispose();
  }
}
