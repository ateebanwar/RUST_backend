import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import '../theme/app_theme.dart';

/// Compact attachment options sheet opened by the composer plus button.
class AttachmentSheet extends StatelessWidget {
  final Function(PlatformFile file) onFileSelected;
  final VoidCallback? onDriveFilesTap;

  const AttachmentSheet({
    super.key,
    required this.onFileSelected,
    this.onDriveFilesTap,
  });

  static Future<void> show(
    BuildContext context, {
    required Function(PlatformFile file) onFileSelected,
    VoidCallback? onDriveFilesTap,
  }) {
    return showModalBottomSheet(
      context: context,
      backgroundColor: AppTheme.darkSurface,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(20)),
      ),
      builder: (ctx) => AttachmentSheet(
        onFileSelected: (f) {
          onFileSelected(f);
          Navigator.pop(ctx);
        },
        onDriveFilesTap: () {
          Navigator.pop(ctx);
          onDriveFilesTap?.call();
        },
      ),
    );
  }

  Future<void> _pickImages(BuildContext context) async {
    try {
      final files = await FilePicker.pickFiles(
        type: FileType.custom,
        allowedExtensions: ['jpg', 'jpeg', 'png', 'webp', 'gif'],
      );
      if (files.isNotEmpty) {
        onFileSelected(files.first);
      }
    } catch (e) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Could not open image picker: $e')),
        );
      }
    }
  }

  Future<void> _pickDocuments(BuildContext context) async {
    try {
      final files = await FilePicker.pickFiles(
        type: FileType.custom,
        allowedExtensions: ['pdf', 'doc', 'docx', 'txt', 'md', 'json', 'csv'],
      );
      if (files.isNotEmpty) {
        onFileSelected(files.first);
      }
    } catch (e) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Could not open document picker: $e')),
        );
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Center(
              child: Container(
                width: 36,
                height: 4,
                margin: const EdgeInsets.only(bottom: 16),
                decoration: BoxDecoration(
                  color: AppTheme.darkBorder,
                  borderRadius: BorderRadius.circular(2),
                ),
              ),
            ),
            const Padding(
              padding: EdgeInsets.symmetric(horizontal: 8, vertical: 4),
              child: Text(
                'Add to chat',
                style: TextStyle(
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                  color: AppTheme.textPrimaryDark,
                ),
              ),
            ),
            const SizedBox(height: 8),
            ListTile(
              leading: Container(
                padding: const EdgeInsets.all(8),
                decoration: BoxDecoration(
                  color: AppTheme.darkSurfaceVariant,
                  borderRadius: BorderRadius.circular(10),
                ),
                child: const Icon(Icons.image_outlined, color: AppTheme.textPrimaryDark, size: 20),
              ),
              title: const Text(
                'Upload Photos & Images',
                style: TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
              ),
              subtitle: const Text(
                'PNG, JPG, WebP',
                style: TextStyle(fontSize: 12, color: AppTheme.textSecondaryDark),
              ),
              onTap: () => _pickImages(context),
            ),
            ListTile(
              leading: Container(
                padding: const EdgeInsets.all(8),
                decoration: BoxDecoration(
                  color: AppTheme.darkSurfaceVariant,
                  borderRadius: BorderRadius.circular(10),
                ),
                child: const Icon(Icons.description_outlined, color: AppTheme.textPrimaryDark, size: 20),
              ),
              title: const Text(
                'Upload Documents',
                style: TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
              ),
              subtitle: const Text(
                'PDF, DOCX, TXT, Markdown',
                style: TextStyle(fontSize: 12, color: AppTheme.textSecondaryDark),
              ),
              onTap: () => _pickDocuments(context),
            ),
            ListTile(
              leading: Container(
                padding: const EdgeInsets.all(8),
                decoration: BoxDecoration(
                  color: AppTheme.darkSurfaceVariant,
                  borderRadius: BorderRadius.circular(10),
                ),
                child: const Icon(Icons.cloud_outlined, color: AppTheme.textPrimaryDark, size: 20),
              ),
              title: const Text(
                'Google Drive Files',
                style: TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
              ),
              subtitle: const Text(
                'Attach from your Drive library',
                style: TextStyle(fontSize: 12, color: AppTheme.textSecondaryDark),
              ),
              onTap: onDriveFilesTap,
            ),
            const SizedBox(height: 8),
          ],
        ),
      ),
    );
  }
}
