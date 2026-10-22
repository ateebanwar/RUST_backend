import 'package:flutter/material.dart';
import 'screens/chat_screen.dart';
import 'screens/login_screen.dart';
import 'services/auth_service.dart';
import 'services/chat_service.dart';
import 'services/drive_service.dart';
import 'services/speech_service.dart';
import 'state/app_state.dart';
import 'theme/app_theme.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();

  final authService = AuthService();
  final chatService = ChatService();
  final driveService = DriveService();
  final speechService = SpeechService();

  final appState = AppState(
    authService: authService,
    chatService: chatService,
    driveService: driveService,
  );

  runApp(
    AIAssistantApp(
      authService: authService,
      appState: appState,
      speechService: speechService,
    ),
  );
}

class AIAssistantApp extends StatefulWidget {
  final AuthService authService;
  final AppState appState;
  final SpeechService speechService;

  const AIAssistantApp({
    super.key,
    required this.authService,
    required this.appState,
    required this.speechService,
  });

  @override
  State<AIAssistantApp> createState() => _AIAssistantAppState();
}

class _AIAssistantAppState extends State<AIAssistantApp> {
  @override
  void initState() {
    super.initState();
    widget.authService.addListener(_onStateChange);
    widget.appState.addListener(_onStateChange);
  }

  void _onStateChange() {
    setState(() {});
  }

  @override
  void dispose() {
    widget.authService.removeListener(_onStateChange);
    widget.appState.removeListener(_onStateChange);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'AI Assistant',
      debugShowCheckedModeBanner: false,
      theme: AppTheme.lightTheme,
      darkTheme: AppTheme.darkTheme,
      themeMode: widget.appState.themeMode, // Dark by default
      home: widget.authService.isAuthenticated
          ? ChatScreen(
              appState: widget.appState,
              speechService: widget.speechService,
            )
          : LoginScreen(
              authService: widget.authService,
            ),
    );
  }
}
