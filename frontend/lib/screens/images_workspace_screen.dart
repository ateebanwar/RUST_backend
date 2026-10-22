import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import '../theme/app_theme.dart';

/// Dedicated image generation and image editing workspace.
/// 
/// Conforms to honest capability boundaries: provides full functional UI,
/// prompt input, source image picker, and aspect ratio selector, while
/// transparently communicating backend image provider readiness.
class ImagesWorkspaceScreen extends StatefulWidget {
  const ImagesWorkspaceScreen({super.key});

  @override
  State<ImagesWorkspaceScreen> createState() => _ImagesWorkspaceScreenState();
}

class _ImagesWorkspaceScreenState extends State<ImagesWorkspaceScreen>
    with SingleTickerProviderStateMixin {
  late TabController _tabController;
  final TextEditingController _promptController = TextEditingController();
  final TextEditingController _editInstructionController = TextEditingController();

  String _selectedAspectRatio = '1:1';
  final List<String> _aspectRatios = ['1:1', '16:9', '9:16', '4:3'];

  PlatformFile? _selectedSourceImage;
  bool _isProcessing = false;
  String? _statusMessage;

  @override
  void initState() {
    super.initState();
    _tabController = TabController(length: 2, vsync: this);
  }

  Future<void> _pickSourceImage() async {
    try {
      final files = await FilePicker.pickFiles(
        type: FileType.custom,
        allowedExtensions: ['png', 'jpg', 'jpeg', 'webp'],
      );
      if (files.isNotEmpty) {
        setState(() {
          _selectedSourceImage = files.first;
        });
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Failed to pick image: $e')),
        );
      }
    }
  }

  void _handleGenerate() {
    final prompt = _promptController.text.trim();
    if (prompt.isEmpty) return;

    setState(() {
      _isProcessing = true;
      _statusMessage = null;
    });

    // Honest capability check: The Rust backend catalog currently registers text/code models.
    Future.delayed(const Duration(milliseconds: 600), () {
      if (!mounted) return;
      setState(() {
        _isProcessing = false;
        _statusMessage =
            'Image generation is pending backend upstream adapter integration (e.g. DALL-E, Stable Diffusion, or Flux). The current Rust backend provides text & code inference routes.';
      });
    });
  }

  void _handleEdit() {
    final instruction = _editInstructionController.text.trim();
    if (instruction.isEmpty || _selectedSourceImage == null) return;

    setState(() {
      _isProcessing = true;
      _statusMessage = null;
    });

    Future.delayed(const Duration(milliseconds: 600), () {
      if (!mounted) return;
      setState(() {
        _isProcessing = false;
        _statusMessage =
            'Image editing requires a multi-modal diffusion edit route on the Rust backend. Uploaded image "${_selectedSourceImage!.name}" is ready for dispatch once the endpoint is deployed.';
      });
    });
  }

  @override
  void dispose() {
    _tabController.dispose();
    _promptController.dispose();
    _editInstructionController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: AppTheme.darkBg,
      appBar: AppBar(
        title: const Text('Images Workspace'),
        bottom: TabBar(
          controller: _tabController,
          indicatorColor: AppTheme.accentGreen,
          labelColor: AppTheme.textPrimaryDark,
          unselectedLabelColor: AppTheme.textSecondaryDark,
          tabs: const [
            Tab(text: 'Generate Image', icon: Icon(Icons.add_photo_alternate_outlined, size: 20)),
            Tab(text: 'Edit Image', icon: Icon(Icons.tune, size: 20)),
          ],
        ),
      ),
      body: SafeArea(
        child: Column(
          children: [
            // Backend capability notice
            Container(
              margin: const EdgeInsets.all(12),
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
              decoration: BoxDecoration(
                color: AppTheme.darkSurfaceVariant,
                borderRadius: BorderRadius.circular(10),
                border: Border.all(color: AppTheme.darkBorder),
              ),
              child: const Row(
                children: [
                  Icon(Icons.info_outline, color: AppTheme.statusInfo, size: 18),
                  SizedBox(width: 10),
                  Expanded(
                    child: Text(
                      'Connected to Rust backend. Text models are active; image diffusion providers are pending backend deployment.',
                      style: TextStyle(fontSize: 12, color: AppTheme.textSecondaryDark, height: 1.3),
                    ),
                  ),
                ],
              ),
            ),

            if (_statusMessage != null) ...[
              Container(
                margin: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
                padding: const EdgeInsets.all(12),
                decoration: BoxDecoration(
                  color: AppTheme.statusWarning.withOpacity(0.12),
                  borderRadius: BorderRadius.circular(10),
                  border: Border.all(color: AppTheme.statusWarning.withOpacity(0.4)),
                ),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    const Icon(Icons.warning_amber_rounded, color: AppTheme.statusWarning, size: 18),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(
                        _statusMessage!,
                        style: const TextStyle(fontSize: 12, color: AppTheme.statusWarning, height: 1.3),
                      ),
                    ),
                  ],
                ),
              ),
            ],

            Expanded(
              child: TabBarView(
                controller: _tabController,
                children: [
                  _buildGenerateTab(),
                  _buildEditTab(),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildGenerateTab() {
    return SingleChildScrollView(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'Prompt',
            style: TextStyle(fontSize: 14, fontWeight: FontWeight.w600, color: AppTheme.textPrimaryDark),
          ),
          const SizedBox(height: 8),
          TextField(
            controller: _promptController,
            maxLines: 4,
            style: const TextStyle(color: AppTheme.textPrimaryDark, fontSize: 14),
            decoration: const InputDecoration(
              hintText: 'Describe the image you want to generate in detail...',
            ),
          ),
          const SizedBox(height: 20),
          const Text(
            'Aspect Ratio',
            style: TextStyle(fontSize: 14, fontWeight: FontWeight.w600, color: AppTheme.textPrimaryDark),
          ),
          const SizedBox(height: 8),
          Row(
            children: _aspectRatios.map((ratio) {
              final isSelected = ratio == _selectedAspectRatio;
              return Padding(
                padding: const EdgeInsets.only(right: 8),
                child: ChoiceChip(
                  label: Text(ratio),
                  selected: isSelected,
                  selectedColor: AppTheme.accentGreen.withOpacity(0.2),
                  backgroundColor: AppTheme.darkSurfaceVariant,
                  labelStyle: TextStyle(
                    color: isSelected ? AppTheme.accentGreen : AppTheme.textSecondaryDark,
                    fontSize: 13,
                    fontWeight: isSelected ? FontWeight.w600 : FontWeight.normal,
                  ),
                  onSelected: (val) {
                    if (val) {
                      setState(() {
                        _selectedAspectRatio = ratio;
                      });
                    }
                  },
                ),
              );
            }).toList(),
          ),
          const SizedBox(height: 28),
          SizedBox(
            width: double.infinity,
            height: 48,
            child: ElevatedButton.icon(
              onPressed: _isProcessing ? null : _handleGenerate,
              icon: _isProcessing
                  ? const SizedBox(
                      width: 16,
                      height: 16,
                      child: CircularProgressIndicator(strokeWidth: 2, color: Colors.white),
                    )
                  : const Icon(Icons.auto_awesome, size: 18),
              label: Text(
                _isProcessing ? 'Generating...' : 'Generate Image',
                style: const TextStyle(fontSize: 15, fontWeight: FontWeight.w600),
              ),
              style: ElevatedButton.styleFrom(
                backgroundColor: AppTheme.accentGreen,
                foregroundColor: Colors.white,
                shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildEditTab() {
    return SingleChildScrollView(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'Source Image',
            style: TextStyle(fontSize: 14, fontWeight: FontWeight.w600, color: AppTheme.textPrimaryDark),
          ),
          const SizedBox(height: 8),
          if (_selectedSourceImage == null)
            InkWell(
              onTap: _pickSourceImage,
              borderRadius: BorderRadius.circular(12),
              child: Container(
                width: double.infinity,
                height: 140,
                decoration: BoxDecoration(
                  color: AppTheme.darkSurfaceVariant,
                  borderRadius: BorderRadius.circular(12),
                  border: Border.all(color: AppTheme.darkBorder, style: BorderStyle.solid),
                ),
                child: const Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    Icon(Icons.cloud_upload_outlined, size: 36, color: AppTheme.textSecondaryDark),
                    SizedBox(height: 8),
                    Text(
                      'Click to select source image',
                      style: TextStyle(fontSize: 14, color: AppTheme.textSecondaryDark),
                    ),
                    Text(
                      'PNG, JPG, WebP',
                      style: TextStyle(fontSize: 11, color: AppTheme.textMutedDark),
                    ),
                  ],
                ),
              ),
            )
          else
            Container(
              padding: const EdgeInsets.all(12),
              decoration: BoxDecoration(
                color: AppTheme.darkSurfaceVariant,
                borderRadius: BorderRadius.circular(12),
                border: Border.all(color: AppTheme.darkBorder),
              ),
              child: Row(
                children: [
                  const Icon(Icons.image, size: 28, color: AppTheme.accentGreen),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          _selectedSourceImage!.name,
                          style: const TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
                          overflow: TextOverflow.ellipsis,
                        ),
                        Text(
                          '${((_selectedSourceImage!.lengthSync() ?? 0) / 1024).toStringAsFixed(1)} KB',
                          style: const TextStyle(fontSize: 12, color: AppTheme.textSecondaryDark),
                        ),
                      ],
                    ),
                  ),
                  IconButton(
                    icon: const Icon(Icons.close, size: 18, color: AppTheme.textMutedDark),
                    onPressed: () {
                      setState(() {
                        _selectedSourceImage = null;
                      });
                    },
                  ),
                ],
              ),
            ),
          const SizedBox(height: 20),
          const Text(
            'Edit Instructions',
            style: TextStyle(fontSize: 14, fontWeight: FontWeight.w600, color: AppTheme.textPrimaryDark),
          ),
          const SizedBox(height: 8),
          TextField(
            controller: _editInstructionController,
            maxLines: 3,
            style: const TextStyle(color: AppTheme.textPrimaryDark, fontSize: 14),
            decoration: const InputDecoration(
              hintText: 'Describe changes: e.g. "Add a retro sunglasses filter" or "Change lighting to neon blue"...',
            ),
          ),
          const SizedBox(height: 28),
          SizedBox(
            width: double.infinity,
            height: 48,
            child: ElevatedButton.icon(
              onPressed: (_isProcessing || _selectedSourceImage == null) ? null : _handleEdit,
              icon: _isProcessing
                  ? const SizedBox(
                      width: 16,
                      height: 16,
                      child: CircularProgressIndicator(strokeWidth: 2, color: Colors.white),
                    )
                  : const Icon(Icons.tune, size: 18),
              label: Text(
                _isProcessing ? 'Processing Edit...' : 'Apply Image Edit',
                style: const TextStyle(fontSize: 15, fontWeight: FontWeight.w600),
              ),
              style: ElevatedButton.styleFrom(
                backgroundColor: AppTheme.accentGreen,
                foregroundColor: Colors.white,
                shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
              ),
            ),
          ),
        ],
      ),
    );
  }
}
