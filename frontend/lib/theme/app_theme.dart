import 'package:flutter/material.dart';

/// ChatGPT Android-inspired dark design system and themes.
class AppTheme {
  // Dark Theme Colors (ChatGPT Android aesthetic)
  static const Color darkBg = Color(0xFF171717);
  static const Color darkSidebar = Color(0xFF171717);
  static const Color darkSurface = Color(0xFF212121);
  static const Color darkSurfaceVariant = Color(0xFF2F2F2F);
  static const Color darkBorder = Color(0xFF2E2F30);
  static const Color darkDivider = Color(0xFF2B2B2B);

  // Text Colors
  static const Color textPrimaryDark = Color(0xFFECECEC);
  static const Color textSecondaryDark = Color(0xFF9E9E9E);
  static const Color textMutedDark = Color(0xFF757575);

  // Status & Accent
  static const Color accentGreen = Color(0xFF10A37F); // subtle ChatGPT emerald
  static const Color statusSuccess = Color(0xFF81C784);
  static const Color statusWarning = Color(0xFFFFB74D);
  static const Color statusError = Color(0xFFEF5350);
  static const Color statusInfo = Color(0xFF64B5F6);

  // Light Theme Colors (for personalization switch)
  static const Color lightBg = Color(0xFFFFFFFF);
  static const Color lightSidebar = Color(0xFFF7F7F8);
  static const Color lightSurface = Color(0xFFF0F0F0);
  static const Color lightSurfaceVariant = Color(0xFFE5E5E5);
  static const Color lightBorder = Color(0xFFE0E0E0);
  static const Color textPrimaryLight = Color(0xFF0D0D0D);
  static const Color textSecondaryLight = Color(0xFF6E6E80);

  /// Default dark theme
  static ThemeData get darkTheme {
    return ThemeData(
      brightness: Brightness.dark,
      scaffoldBackgroundColor: darkBg,
      primaryColor: Colors.white,
      canvasColor: darkSidebar,
      colorScheme: const ColorScheme.dark(
        surface: darkSurface,
        primary: Colors.white,
        secondary: accentGreen,
        error: statusError,
        onSurface: textPrimaryDark,
      ),
      appBarTheme: const AppBarTheme(
        backgroundColor: darkBg,
        elevation: 0,
        scrolledUnderElevation: 0,
        centerTitle: true,
        titleTextStyle: TextStyle(
          color: textPrimaryDark,
          fontSize: 17,
          fontWeight: FontWeight.w600,
          letterSpacing: -0.2,
        ),
        iconTheme: IconThemeData(color: textPrimaryDark, size: 22),
      ),
      drawerTheme: const DrawerThemeData(
        backgroundColor: darkSidebar,
        elevation: 1,
        width: 300,
        surfaceTintColor: Colors.transparent,
      ),
      dividerTheme: const DividerThemeData(
        color: darkDivider,
        thickness: 1,
        space: 1,
      ),
      textTheme: const TextTheme(
        headlineMedium: TextStyle(
          color: textPrimaryDark,
          fontSize: 24,
          fontWeight: FontWeight.w600,
          letterSpacing: -0.5,
        ),
        titleLarge: TextStyle(
          color: textPrimaryDark,
          fontSize: 18,
          fontWeight: FontWeight.w600,
          letterSpacing: -0.3,
        ),
        titleMedium: TextStyle(
          color: textPrimaryDark,
          fontSize: 15,
          fontWeight: FontWeight.w500,
        ),
        bodyLarge: TextStyle(
          color: textPrimaryDark,
          fontSize: 15,
          height: 1.5,
          letterSpacing: -0.1,
        ),
        bodyMedium: TextStyle(
          color: textPrimaryDark,
          fontSize: 14,
          height: 1.45,
        ),
        bodySmall: TextStyle(
          color: textSecondaryDark,
          fontSize: 12,
        ),
        labelLarge: TextStyle(
          color: textPrimaryDark,
          fontSize: 14,
          fontWeight: FontWeight.w500,
        ),
      ),
      iconTheme: const IconThemeData(
        color: textSecondaryDark,
        size: 20,
      ),
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        fillColor: darkSurfaceVariant,
        contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(24),
          borderSide: BorderSide.none,
        ),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(24),
          borderSide: BorderSide.none,
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(24),
          borderSide: const BorderSide(color: darkBorder, width: 1),
        ),
        hintStyle: const TextStyle(
          color: textMutedDark,
          fontSize: 15,
        ),
      ),
    );
  }

  /// Optional light theme
  static ThemeData get lightTheme {
    return ThemeData(
      brightness: Brightness.light,
      scaffoldBackgroundColor: lightBg,
      primaryColor: Colors.black,
      canvasColor: lightSidebar,
      colorScheme: const ColorScheme.light(
        surface: lightSurface,
        primary: Colors.black,
        secondary: accentGreen,
        error: statusError,
        onSurface: textPrimaryLight,
      ),
      appBarTheme: const AppBarTheme(
        backgroundColor: lightBg,
        elevation: 0,
        scrolledUnderElevation: 0,
        centerTitle: true,
        titleTextStyle: TextStyle(
          color: textPrimaryLight,
          fontSize: 17,
          fontWeight: FontWeight.w600,
          letterSpacing: -0.2,
        ),
        iconTheme: IconThemeData(color: textPrimaryLight, size: 22),
      ),
      drawerTheme: const DrawerThemeData(
        backgroundColor: lightSidebar,
        elevation: 1,
        width: 300,
        surfaceTintColor: Colors.transparent,
      ),
      dividerTheme: const DividerThemeData(
        color: lightBorder,
        thickness: 1,
        space: 1,
      ),
      textTheme: const TextTheme(
        headlineMedium: TextStyle(
          color: textPrimaryLight,
          fontSize: 24,
          fontWeight: FontWeight.w600,
          letterSpacing: -0.5,
        ),
        titleLarge: TextStyle(
          color: textPrimaryLight,
          fontSize: 18,
          fontWeight: FontWeight.w600,
          letterSpacing: -0.3,
        ),
        bodyLarge: TextStyle(
          color: textPrimaryLight,
          fontSize: 15,
          height: 1.5,
        ),
        bodyMedium: TextStyle(
          color: textPrimaryLight,
          fontSize: 14,
          height: 1.45,
        ),
        bodySmall: TextStyle(
          color: textSecondaryLight,
          fontSize: 12,
        ),
      ),
    );
  }
}
