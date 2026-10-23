import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import '../config/app_config.dart';
import '../services/auth_service.dart';
import '../theme/app_theme.dart';
import '../widgets/google_sign_in_button.dart';

/// Clean Google Sign-In screen for ChatGPT-inspired mobile interface.
class LoginScreen extends StatefulWidget {
  final AuthService authService;

  const LoginScreen({super.key, required this.authService});

  @override
  State<LoginScreen> createState() => _LoginScreenState();
}

class _LoginScreenState extends State<LoginScreen> {
  final TextEditingController _clientIdController =
      TextEditingController(text: AppConfig.googleClientId);
  final TextEditingController _customTokenController = TextEditingController();
  final TextEditingController _customDriveTokenController = TextEditingController();
  bool _showAdvanced = false;

  @override
  void initState() {
    super.initState();
    widget.authService.ensureInitialized();
  }

  @override
  void dispose() {
    _clientIdController.dispose();
    _customTokenController.dispose();
    _customDriveTokenController.dispose();
    super.dispose();
  }

  /// Initiate genuine Google Sign-In flow
  Future<void> _handleGoogleSignIn() async {
    final configuredId = _clientIdController.text.trim();
    if (configuredId.isNotEmpty) {
      AppConfig.googleClientId = configuredId;
    }
    await widget.authService.signInWithGoogle();
  }

  /// Launch isolated offline developer session for automated widget testing
  Future<void> _handleDevSession() async {
    await widget.authService.launchDevSession(
      simulatedName: 'Ateeb (Test)',
      simulatedEmail: 'ateeb.developer@gmail.com',
    );
  }

  /// Sign in using custom credentials (e.g. backend PAT / API key or manual OAuth tokens)
  Future<void> _handleCustomTokenSignIn() async {
    final idToken = _customTokenController.text.trim();
    final driveToken = _customDriveTokenController.text.trim();
    if (idToken.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('Please enter an ID token or API key'),
          backgroundColor: AppTheme.statusError,
        ),
      );
      return;
    }
    await widget.authService.signInWithCustomTokens(
      idToken: idToken,
      driveToken: driveToken.isNotEmpty ? driveToken : null,
    );
  }

  @override
  Widget build(BuildContext context) {
    final isLoading = widget.authService.isLoading;
    final lastError = widget.authService.lastError;

    return Scaffold(
      backgroundColor: AppTheme.darkBg,
      body: SafeArea(
        child: Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.symmetric(horizontal: 28),
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 400),
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  // App Icon
                  Container(
                    width: 64,
                    height: 64,
                    decoration: BoxDecoration(
                      color: AppTheme.darkSurfaceVariant,
                      shape: BoxShape.circle,
                      border: Border.all(color: AppTheme.darkBorder, width: 1),
                    ),
                    child: const Icon(
                      Icons.auto_awesome,
                      color: Colors.white,
                      size: 32,
                    ),
                  ),
                  const SizedBox(height: 24),

                  // Title & Description
                  const Text(
                    'Welcome back',
                    style: TextStyle(
                      fontSize: 24,
                      fontWeight: FontWeight.bold,
                      color: AppTheme.textPrimaryDark,
                      letterSpacing: -0.5,
                    ),
                  ),
                  const SizedBox(height: 8),
                  const Text(
                    'Sign in with Google to access your conversations and synced Google Drive storage.',
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      fontSize: 14,
                      color: AppTheme.textSecondaryDark,
                      height: 1.4,
                    ),
                  ),
                  const SizedBox(height: 36),

                  // Error banner if any
                  if (lastError != null) ...[
                    Container(
                      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
                      margin: const EdgeInsets.only(bottom: 20),
                      decoration: BoxDecoration(
                        color: AppTheme.statusError.withOpacity(0.12),
                        borderRadius: BorderRadius.circular(10),
                        border: Border.all(color: AppTheme.statusError.withOpacity(0.3)),
                      ),
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          const Padding(
                            padding: EdgeInsets.only(top: 2),
                            child: Icon(Icons.error_outline, color: AppTheme.statusError, size: 18),
                          ),
                          const SizedBox(width: 8),
                          Expanded(
                            child: Text(
                              lastError,
                              style: const TextStyle(fontSize: 13, color: AppTheme.statusError),
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],

                  // Primary Google Sign-In Button
                  buildGoogleSignInButton(
                    onPressed: _handleGoogleSignIn,
                    isLoading: isLoading,
                  ),
                  const SizedBox(height: 16),

                  // Isolated developer test button for automated testing and offline verification
                  if (kDebugMode) ...[
                    SizedBox(
                      width: double.infinity,
                      height: 46,
                      child: OutlinedButton(
                        onPressed: isLoading ? null : _handleDevSession,
                        style: OutlinedButton.styleFrom(
                          foregroundColor: AppTheme.textPrimaryDark,
                          side: const BorderSide(color: AppTheme.darkBorder),
                          shape: RoundedRectangleBorder(
                            borderRadius: BorderRadius.circular(25),
                          ),
                        ),
                        child: const Text(
                          'Launch Dev Session (Offline / Edge Test)',
                          style: TextStyle(fontSize: 13, fontWeight: FontWeight.w500),
                        ),
                      ),
                    ),
                    const SizedBox(height: 20),
                  ],

                  // Advanced custom token toggle
                  TextButton(
                    onPressed: () {
                      setState(() {
                        _showAdvanced = !_showAdvanced;
                      });
                    },
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Text(
                          _showAdvanced ? 'Hide Token Settings' : 'Custom OAuth / Drive Tokens',
                          style: const TextStyle(fontSize: 12, color: AppTheme.textMutedDark),
                        ),
                        Icon(
                          _showAdvanced ? Icons.keyboard_arrow_up : Icons.keyboard_arrow_down,
                          size: 16,
                          color: AppTheme.textMutedDark,
                        ),
                      ],
                    ),
                  ),

                  if (_showAdvanced) ...[
                    const SizedBox(height: 12),
                    TextField(
                      controller: _clientIdController,
                      style: const TextStyle(fontSize: 13, color: AppTheme.textPrimaryDark),
                      decoration: const InputDecoration(
                        labelText: 'Google Web Client ID',
                        hintText: 'e.g. 12345...apps.googleusercontent.com',
                        labelStyle: TextStyle(fontSize: 12, color: AppTheme.textSecondaryDark),
                      ),
                      onChanged: (val) {
                        AppConfig.googleClientId = val.trim();
                      },
                    ),
                    const SizedBox(height: 12),
                    TextField(
                      controller: _customTokenController,
                      style: const TextStyle(fontSize: 13, color: AppTheme.textPrimaryDark),
                      decoration: const InputDecoration(
                        labelText: 'Google ID Token / API Key (Optional)',
                        hintText: 'Bearer <id_token>',
                        labelStyle: TextStyle(fontSize: 12, color: AppTheme.textSecondaryDark),
                      ),
                    ),
                    const SizedBox(height: 12),
                    TextField(
                      controller: _customDriveTokenController,
                      style: const TextStyle(fontSize: 13, color: AppTheme.textPrimaryDark),
                      decoration: const InputDecoration(
                        labelText: 'Google Drive Access Token (Optional)',
                        hintText: 'X-Drive-Token <oauth_access_token>',
                        labelStyle: TextStyle(fontSize: 12, color: AppTheme.textSecondaryDark),
                      ),
                    ),
                    const SizedBox(height: 12),
                    SizedBox(
                      width: double.infinity,
                      height: 40,
                      child: ElevatedButton(
                        onPressed: isLoading ? null : _handleCustomTokenSignIn,
                        style: ElevatedButton.styleFrom(
                          backgroundColor: AppTheme.darkSurfaceVariant,
                          foregroundColor: AppTheme.textPrimaryDark,
                          elevation: 0,
                          shape: RoundedRectangleBorder(
                            borderRadius: BorderRadius.circular(20),
                          ),
                        ),
                        child: const Text('Sign in with Custom Credentials', style: TextStyle(fontSize: 13)),
                      ),
                    ),
                  ],

                  const SizedBox(height: 36),
                  const Text(
                    'All user data and conversation histories are stored exclusively in your Google Drive AppData folder. No local database is maintained.',
                    textAlign: TextAlign.center,
                    style: TextStyle(fontSize: 11, color: AppTheme.textMutedDark, height: 1.4),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
