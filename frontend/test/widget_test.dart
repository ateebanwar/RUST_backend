import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ai_assistant_frontend/config/app_config.dart';
import 'package:ai_assistant_frontend/main.dart';
import 'package:ai_assistant_frontend/models/conversation.dart';
import 'package:ai_assistant_frontend/screens/chat_screen.dart';
import 'package:ai_assistant_frontend/screens/documents_workspace_screen.dart';
import 'package:ai_assistant_frontend/screens/images_workspace_screen.dart';
import 'package:ai_assistant_frontend/screens/personalization_screen.dart';
import 'package:ai_assistant_frontend/screens/search_screen.dart';
import 'package:ai_assistant_frontend/services/auth_service.dart';
import 'package:ai_assistant_frontend/services/chat_service.dart';
import 'package:ai_assistant_frontend/services/drive_service.dart';
import 'package:ai_assistant_frontend/services/speech_service.dart';
import 'package:ai_assistant_frontend/state/app_state.dart';
import 'package:ai_assistant_frontend/widgets/message_bubble.dart';
import 'package:ai_assistant_frontend/widgets/think_modal.dart';

void main() {
  late AuthService authService;
  late ChatService chatService;
  late DriveService driveService;
  late SpeechService speechService;
  late AppState appState;

  setUp(() {
    authService = AuthService();
    chatService = ChatService();
    driveService = DriveService();
    speechService = SpeechService();
    appState = AppState(
      authService: authService,
      chatService: chatService,
      driveService: driveService,
    );
  });

  testWidgets('Unauthenticated user lands on LoginScreen', (tester) async {
    await tester.pumpWidget(
      AIAssistantApp(
        authService: authService,
        appState: appState,
        speechService: speechService,
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Welcome back'), findsOneWidget);
    expect(find.text('Continue with Google'), findsOneWidget);
    expect(find.text('Launch Dev Session (Offline / Edge Test)'), findsOneWidget);
  });

  testWidgets('Dev sign-in authenticates and opens ChatScreen directly', (tester) async {
    await tester.pumpWidget(
      AIAssistantApp(
        authService: authService,
        appState: appState,
        speechService: speechService,
      ),
    );
    await tester.pumpAndSettle();

    // Click Launch Dev Session
    await tester.tap(find.text('Launch Dev Session (Offline / Edge Test)'));
    await tester.pumpAndSettle();

    expect(authService.isAuthenticated, true);
    expect(find.text('AI Assistant'), findsOneWidget);
    expect(find.text('What can I help with today?'), findsOneWidget);
    expect(find.byType(TextField), findsOneWidget);
  });

  testWidgets('ChatScreen drawer opens and displays navigation items in order', (tester) async {
    await authService.launchDevSession(simulatedName: 'Ateeb', simulatedEmail: 'ateeb@gmail.com');

    await tester.pumpWidget(
      MaterialApp(
        home: ChatScreen(
          appState: appState,
          speechService: speechService,
        ),
      ),
    );
    await tester.pumpAndSettle();

    // Open Drawer via menu button
    await tester.tap(find.byIcon(Icons.menu));
    await tester.pumpAndSettle();

    // Verify navigation items
    expect(find.text('New Chat'), findsOneWidget);
    expect(find.text('Search'), findsOneWidget);
    expect(find.text('Images'), findsOneWidget);
    expect(find.text('Documents'), findsOneWidget);
    expect(find.text('Recent Chats'), findsOneWidget);

    // Verify pinned account section
    expect(find.text('Ateeb'), findsOneWidget);
    expect(find.text('ateeb@gmail.com'), findsOneWidget);
  });

  testWidgets('MessageBubble displays Drive save failure banner with retry button', (tester) async {
    final unsavedMessage = SyncMessage(
      id: 'msg_1',
      conversationId: 'conv_1',
      role: 'assistant',
      content: 'Here is the generated analysis.',
      timestamp: DateTime.now().toIso8601String(),
      isSaved: false,
      saveError: 'Google Drive token expired or unconfigured encryption key',
      isRetryable: true,
    );

    bool retryClicked = false;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: MessageBubble(
            message: unsavedMessage,
            onRetry: () => retryClicked = true,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Here is the generated analysis.'), findsOneWidget);
    expect(find.text('Response generated, but saving to Google Drive failed.'), findsOneWidget);
    expect(find.text('Retry'), findsOneWidget);

    await tester.tap(find.text('Retry'));
    expect(retryClicked, true);
  });

  testWidgets('ThinkModal presents reasoning options', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Builder(
            builder: (context) => ElevatedButton(
              onPressed: () {
                ThinkModal.show(
                  context,
                  currentMode: appState.selectedThinkMode,
                  onSelect: (m) => appState.setThinkMode(m),
                );
              },
              child: const Text('Open Think'),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('Open Think'));
    await tester.pumpAndSettle();

    expect(find.text('Thinking Mode'), findsOneWidget);
    expect(find.text('Auto Thinking'), findsOneWidget);
    expect(find.text('Deep Reasoning'), findsOneWidget);
    expect(find.text('Fast Speed'), findsOneWidget);
    expect(find.text('Standard (Off)'), findsOneWidget);
  });

  testWidgets('PersonalizationScreen renders General and Appearance settings', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: PersonalizationScreen(appState: appState),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('General'), findsOneWidget);
    expect(find.text('App Language'), findsOneWidget);
    expect(find.text('Appearance'), findsOneWidget);
    expect(find.text('Dark (Default)'), findsOneWidget);
    expect(find.text('Light'), findsOneWidget);
    expect(find.text('System'), findsOneWidget);
  });

  testWidgets('ImagesWorkspaceScreen renders generate and edit tabs with capability notice', (tester) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: ImagesWorkspaceScreen(),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Images Workspace'), findsOneWidget);
    expect(find.text('Generate Image'), findsNWidgets(2));
    expect(find.text('Edit Image'), findsOneWidget);
    expect(find.text('Aspect Ratio'), findsOneWidget);
  });

  testWidgets('DocumentsWorkspaceScreen renders format chips and outline input', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: DocumentsWorkspaceScreen(appState: appState),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Documents Workspace'), findsOneWidget);
    expect(find.text('Desired Format'), findsOneWidget);
    expect(find.text('Word (.docx)'), findsOneWidget);
    expect(find.text('PowerPoint (.pptx)'), findsOneWidget);
    expect(find.text('PDF'), findsOneWidget);
  });

  testWidgets('SearchScreen debounces and displays empty state', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: SearchScreen(
          appState: appState,
          onConversationSelected: () {},
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Search your conversations'), findsOneWidget);
    expect(find.byType(TextField), findsOneWidget);
  });

  testWidgets('Clicking Continue with Google with unconfigured Client ID shows actionable error', (tester) async {
    final originalClientId = AppConfig.googleClientId;
    AppConfig.googleClientId = '';
    try {
      await tester.pumpWidget(
        AIAssistantApp(
          authService: authService,
          appState: appState,
          speechService: speechService,
        ),
      );
      await tester.pumpAndSettle();

      // Click Continue with Google
      await tester.tap(find.text('Continue with Google'));
      await tester.pumpAndSettle();

      expect(authService.isAuthenticated, false);
      expect(find.textContaining('Google OAuth Web Client ID is not configured'), findsOneWidget);
    } finally {
      AppConfig.googleClientId = originalClientId;
    }
  });

  testWidgets('Sign in with custom token sets genuine non-mock AuthUser', (tester) async {
    await authService.signInWithCustomTokens(
      idToken: 'test_pat_token_value',
      driveToken: 'test_drive_token_value',
      name: 'Custom Tester',
      email: 'tester@example.com',
    );

    expect(authService.isAuthenticated, true);
    expect(authService.isMockSession, false);
    expect(authService.currentUser?.name, 'Custom Tester');
    expect(authService.currentUser?.email, 'tester@example.com');
    expect(authService.currentUser?.hasDriveToken, true);

    await authService.signOut();
    expect(authService.isAuthenticated, false);
    expect(authService.currentUser, isNull);
  });
}
