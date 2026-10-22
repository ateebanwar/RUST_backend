import 'package:flutter/material.dart';
import '../state/app_state.dart';
import '../theme/app_theme.dart';

/// Minimal personalization settings screen.
/// 
/// Strictly contains:
/// - General (Application Language)
/// - Appearance (Dark Theme [Default], Light Theme, System Theme)
/// 
/// Strictly excludes Notifications, separate Profile page, or Library settings.
/// Stores preferences in-memory per architectural storage constraints.
class PersonalizationScreen extends StatelessWidget {
  final AppState appState;

  const PersonalizationScreen({super.key, required this.appState});

  static const List<String> _languages = [
    'English (US)',
    'Spanish (Español)',
    'French (Français)',
    'German (Deutsch)',
    'Chinese (中文)',
    'Japanese (日本語)',
  ];

  @override
  Widget build(BuildContext context) {
    final currentTheme = appState.themeMode;
    final currentLang = appState.selectedLanguage;

    return Scaffold(
      backgroundColor: AppTheme.darkBg,
      appBar: AppBar(
        title: const Text('Personalization'),
      ),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
          children: [
            // General Section
            const Padding(
              padding: EdgeInsets.symmetric(vertical: 8, horizontal: 4),
              child: Text(
                'General',
                style: TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                  color: AppTheme.textMutedDark,
                  letterSpacing: 0.5,
                ),
              ),
            ),
            Material(
              color: AppTheme.darkSurfaceVariant,
              borderRadius: BorderRadius.circular(12),
              clipBehavior: Clip.antiAlias,
              child: ListTile(
                leading: const Icon(Icons.language, color: AppTheme.textPrimaryDark, size: 20),
                title: const Text(
                  'App Language',
                  style: TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
                ),
                trailing: DropdownButton<String>(
                  value: currentLang,
                  underline: const SizedBox(),
                  dropdownColor: AppTheme.darkSurface,
                  style: const TextStyle(fontSize: 13, color: AppTheme.textSecondaryDark),
                  icon: const Icon(Icons.keyboard_arrow_down, size: 18, color: AppTheme.textSecondaryDark),
                  items: _languages.map((lang) {
                    return DropdownMenuItem<String>(
                      value: lang,
                      child: Text(lang),
                    );
                  }).toList(),
                  onChanged: (val) {
                    if (val != null) {
                      appState.setLanguage(val);
                    }
                  },
                ),
              ),
            ),
            const SizedBox(height: 24),

            // Appearance Section
            const Padding(
              padding: EdgeInsets.symmetric(vertical: 8, horizontal: 4),
              child: Text(
                'Appearance',
                style: TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                  color: AppTheme.textMutedDark,
                  letterSpacing: 0.5,
                ),
              ),
            ),
            Material(
              color: AppTheme.darkSurfaceVariant,
              borderRadius: BorderRadius.circular(12),
              clipBehavior: Clip.antiAlias,
              child: Column(
                children: [
                  RadioListTile<ThemeMode>(
                    value: ThemeMode.dark,
                    groupValue: currentTheme,
                    title: const Text(
                      'Dark (Default)',
                      style: TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
                    ),
                    subtitle: const Text(
                      'ChatGPT Android dark charcoal aesthetic',
                      style: TextStyle(fontSize: 12, color: AppTheme.textSecondaryDark),
                    ),
                    activeColor: AppTheme.accentGreen,
                    onChanged: (val) {
                      if (val != null) appState.setThemeMode(val);
                    },
                  ),
                  const Divider(color: AppTheme.darkBorder, height: 1),
                  RadioListTile<ThemeMode>(
                    value: ThemeMode.light,
                    groupValue: currentTheme,
                    title: const Text(
                      'Light',
                      style: TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
                    ),
                    activeColor: AppTheme.accentGreen,
                    onChanged: (val) {
                      if (val != null) appState.setThemeMode(val);
                    },
                  ),
                  const Divider(color: AppTheme.darkBorder, height: 1),
                  RadioListTile<ThemeMode>(
                    value: ThemeMode.system,
                    groupValue: currentTheme,
                    title: const Text(
                      'System',
                      style: TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
                    ),
                    subtitle: const Text(
                      'Matches device system settings',
                      style: TextStyle(fontSize: 12, color: AppTheme.textSecondaryDark),
                    ),
                    activeColor: AppTheme.accentGreen,
                    onChanged: (val) {
                      if (val != null) appState.setThemeMode(val);
                    },
                  ),
                ],
              ),
            ),
            const SizedBox(height: 24),

            // In-memory notice footnote
            const Padding(
              padding: EdgeInsets.symmetric(horizontal: 8),
              child: Text(
                'Preferences are preserved in memory during your active session. User chat data and attachments remain permanently in Google Drive.',
                style: TextStyle(fontSize: 11, color: AppTheme.textMutedDark, height: 1.4),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
